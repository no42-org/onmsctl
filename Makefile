.PHONY: help build test verify fmt clippy deny fuzz-check fuzz lint-actions licenses licenses-check install-tools install-cargo-deny install-cargo-about install-cargo-cyclonedx install-cargo-fuzz install-actionlint install-zizmor release-build sbom integration schema docker clean

# Self-documenting: annotate each user-facing target with `## description`
# and it shows up in `make help`. Sorted in declaration order.
help:  ## Show available make targets
	@awk 'BEGIN {FS = ":.*?## "} \
	  /^[a-zA-Z][a-zA-Z0-9_-]*:.*?## / { \
	    printf "  %-16s %s\n", $$1, $$2 \
	  }' $(MAKEFILE_LIST)

build:  ## Compile the workspace (all crates, all targets)
	cargo build --workspace --all-targets

test:  ## Run the workspace unit and doc tests
	cargo test --workspace --all-targets

fmt:  ## Check formatting (rustfmt --check; does not modify files)
	cargo fmt --all -- --check

clippy:  ## Run clippy across the workspace, warnings fail
	cargo clippy --workspace --all-targets -- -D warnings

deny: install-cargo-deny  ## Check advisories, bans, licenses, sources (cargo-deny)
	cargo deny check

verify: fmt clippy build test deny  ## Full quality gate: fmt + clippy + build + test + deny

# Fuzz harnesses live in their own workspace under fuzz/ (see fuzz/Cargo.toml).
# Running them needs nightly + cargo-fuzz; linting them does not. The
# check runs as the `fuzz-check` job in gates.yml so a parser API change
# cannot silently break the harnesses.
#
# No --config and no fuzz/deny.toml: cargo-deny walks up from the manifest
# directory and loads the root deny.toml on its own (verified on 0.19.4
# and 0.20.2, which disagree on where --config goes), and that is what
# makes the root NCSA exception live. `bans` is skipped: the fuzz lockfile
# resolves independently of the root one, so its duplicate-version set
# never matches the root skip list, and duplicate versions in a dev-only
# fuzz build are not a shipping concern.
fuzz-check: install-cargo-deny  ## fmt + clippy + deny the fuzz harnesses on the pinned stable toolchain
	cargo fmt --manifest-path fuzz/Cargo.toml -- --check
	cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
	cargo deny --manifest-path fuzz/Cargo.toml check advisories licenses sources

# Run one fuzz target for FUZZ_SECS seconds (default 60). Targets are the
# [[bin]] names in fuzz/Cargo.toml. The nightly check comes first so a
# machine without nightly fails in a second instead of after compiling
# cargo-fuzz; cargo-fuzz itself is installed on demand like the other
# tools. The nightly toolchain is not, because that is a rustup decision
# the developer should make, so we only check and explain.
#
# -timeout caps one input at FUZZ_INPUT_TIMEOUT seconds. libFuzzer's default
# is 1200 s, which would let a hang like the one this harness found run
# past a whole FUZZ_SECS budget unreported.
#
# The run starts from fuzz/seeds/<target>/ (committed, hand-written inputs
# that reach the interesting branches) and, for the two YAML targets, the
# real documents under examples/. libFuzzer reads seed directories and
# writes new inputs only to the first directory, fuzz/corpus/<target>/,
# which is gitignored.
FUZZ_TARGET ?= parse_documents
FUZZ_SECS ?= 60
FUZZ_INPUT_TIMEOUT ?= 30
FUZZ_CORPUS := fuzz/corpus/$(FUZZ_TARGET)
FUZZ_SEEDS := fuzz/seeds/$(FUZZ_TARGET) $(if $(filter parse_documents event_source_from_yaml,$(FUZZ_TARGET)),examples)
fuzz:  ## Run a fuzz target on nightly (FUZZ_TARGET=parse_documents FUZZ_SECS=60)
	@rustup run nightly rustc --version > /dev/null 2>&1 || { \
	  echo "fuzz: no nightly toolchain. Run: rustup toolchain install nightly" >&2; \
	  echo "fuzz: a nightly older than the workspace rust-version fails later with a clear cargo error; fix with: rustup update nightly" >&2; \
	  exit 1; \
	}
	@$(MAKE) --no-print-directory install-cargo-fuzz
	@mkdir -p '$(FUZZ_CORPUS)'
	cargo +nightly fuzz run '$(FUZZ_TARGET)' '$(FUZZ_CORPUS)' $(FUZZ_SEEDS) -- '-max_total_time=$(FUZZ_SECS)' '-timeout=$(FUZZ_INPUT_TIMEOUT)'

# Lint the workflows themselves. actionlint covers syntax, expressions and
# embedded shell (via shellcheck); zizmor covers the security rules this
# project otherwise enforces by hand — SHA pinning, least-privilege
# permissions, template injection, and credential persistence.
#
# Run as its own CI job rather than folded into `verify`, so a workflow nit
# is not reported as a Rust failure.
lint-actions: install-actionlint install-zizmor  ## Lint .github/workflows (actionlint + zizmor)
	$(ACTIONLINT)
	$(ZIZMOR) .github/workflows/

licenses: install-cargo-about  ## Regenerate THIRD-PARTY-LICENSES.md from the dep tree
	# Atomic write: failure leaves the existing file untouched.
	cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.md.tmp about.hbs && \
		mv THIRD-PARTY-LICENSES.md.tmp THIRD-PARTY-LICENSES.md

# Dependabot bumps Cargo.lock but never runs `make licenses`, so the
# committed report drifts silently. This regenerates to a scratch file and
# diffs — the committed file is never touched — and runs as the
# `licenses-drift` job in gates.yml so the drift fails the PR that
# introduces it, not the next release's checklist.
licenses-check: install-cargo-about  ## Fail if THIRD-PARTY-LICENSES.md is stale vs the dep tree
	@cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.md.check about.hbs
	@if diff -q THIRD-PARTY-LICENSES.md THIRD-PARTY-LICENSES.md.check > /dev/null; then \
	  rm -f THIRD-PARTY-LICENSES.md.check; \
	  echo "THIRD-PARTY-LICENSES.md is current"; \
	else \
	  rm -f THIRD-PARTY-LICENSES.md.check; \
	  echo "THIRD-PARTY-LICENSES.md is stale — run 'make licenses' and commit the diff." >&2; \
	  exit 1; \
	fi

# Build the distroless OCI image for the host architecture. CI builds the
# multi-arch (amd64+arm64) image via .github/workflows/docker.yml; this target
# is the local single-arch equivalent. Override IMAGE to retag.
IMAGE ?= onmsctl:dev
docker:  ## Build the distroless OCI image for the host arch (IMAGE=onmsctl:dev)
	docker build -t $(IMAGE) .

clean:  ## Remove the cargo target directories (root and fuzz/)
	cargo clean
	cargo clean --manifest-path fuzz/Cargo.toml

install-tools: install-cargo-deny install-cargo-about install-cargo-fuzz

install-cargo-deny:
	@command -v cargo-deny > /dev/null 2>&1 || cargo install --locked cargo-deny

# Pinned: the licenses-drift gate diffs a fresh regeneration against the
# committed THIRD-PARTY-LICENSES.md, so every regeneration — local or CI —
# must come from the same cargo-about version, or formatting/content
# differences between releases of the tool read as license drift.
CARGO_ABOUT_VERSION ?= 0.9.1

install-cargo-about:
	@installed="$$(cargo about --version 2> /dev/null | awk '{print $$2}')"; \
	test "$$installed" = "$(CARGO_ABOUT_VERSION)" || \
	  cargo install --locked --features=cli cargo-about --version $(CARGO_ABOUT_VERSION)

install-cargo-cyclonedx:
	@command -v cargo-cyclonedx > /dev/null 2>&1 || cargo install --locked cargo-cyclonedx

install-cargo-fuzz:
	@command -v cargo-fuzz > /dev/null 2>&1 || cargo install --locked cargo-fuzz

# actionlint and zizmor are fetched as pinned upstream release binaries
# into .bin/ rather than built from source. actionlint has no crate (it is
# Go), and building zizmor from source is not possible here at all: it
# needs a newer rustc than the toolchain this workspace pins, so
# `cargo install` fails against rust-toolchain.toml. Prebuilt binaries
# avoid both problems and keep CI fast.
BIN_DIR := $(CURDIR)/.bin

UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)

ACTIONLINT_VERSION ?= 1.7.12
ZIZMOR_VERSION ?= 1.28.0

# actionlint uses Go-style os/arch; zizmor uses Rust target triples.
ifeq ($(UNAME_S),Darwin)
  ACTIONLINT_OS := darwin
  ZIZMOR_TARGET_OS := apple-darwin
else
  ACTIONLINT_OS := linux
  ZIZMOR_TARGET_OS := unknown-linux-gnu
endif
ifeq ($(UNAME_M),x86_64)
  ACTIONLINT_ARCH := amd64
  ZIZMOR_ARCH := x86_64
else
  ACTIONLINT_ARCH := arm64
  ZIZMOR_ARCH := aarch64
endif

ACTIONLINT := $(shell command -v actionlint 2> /dev/null || echo $(BIN_DIR)/actionlint)
ZIZMOR := $(shell command -v zizmor 2> /dev/null || echo $(BIN_DIR)/zizmor)

install-actionlint:
	@test -x "$(ACTIONLINT)" || { \
	  mkdir -p $(BIN_DIR); \
	  echo "fetching actionlint $(ACTIONLINT_VERSION) ($(ACTIONLINT_OS)/$(ACTIONLINT_ARCH))"; \
	  curl -fsSL "https://github.com/rhysd/actionlint/releases/download/v$(ACTIONLINT_VERSION)/actionlint_$(ACTIONLINT_VERSION)_$(ACTIONLINT_OS)_$(ACTIONLINT_ARCH).tar.gz" \
	    | tar -xz -C $(BIN_DIR) actionlint; \
	}

install-zizmor:
	@test -x "$(ZIZMOR)" || { \
	  mkdir -p $(BIN_DIR); \
	  echo "fetching zizmor $(ZIZMOR_VERSION) ($(ZIZMOR_ARCH)-$(ZIZMOR_TARGET_OS))"; \
	  curl -fsSL "https://github.com/zizmorcore/zizmor/releases/download/v$(ZIZMOR_VERSION)/zizmor-$(ZIZMOR_ARCH)-$(ZIZMOR_TARGET_OS).tar.gz" \
	    | tar -xz -C $(BIN_DIR) zizmor; \
	}

# Release-only targets. CI invokes these from .github/workflows/release.yml
# so the local developer command and the CI command stay in sync.

# Build a stripped, optimized binary for $(TARGET). The release workflow
# adds the target via `rustup target add` before invoking us.
release-build:  ## Build a stripped release binary for TARGET=<triple>
	@test -n "$(TARGET)" || (echo "release-build: TARGET=<triple> required" >&2; exit 1)
	cargo build --release --bin onmsctl --target $(TARGET)

# Generate per-crate CycloneDX SBOMs at sbom/<crate>.cdx.json.
#
# cargo-cyclonedx writes one `<crate>.cdx.json` next to each member's
# Cargo.toml. Default filenames already disambiguate — `onmsctl.cdx.json`,
# `onmsctl-core.cdx.json`, `onmsctl-eventconf.cdx.json`. The previous
# `--override-filename onmsctl` flag forced every crate to write to the
# same name, causing the follow-up `mv` to clobber two of three SBOMs.
sbom: install-cargo-cyclonedx  ## Generate per-crate CycloneDX SBOMs under sbom/
	@mkdir -p sbom
	cargo cyclonedx --format json --spec-version 1.5
	@find . -maxdepth 3 -name '*.cdx.json' -not -path './sbom/*' -exec mv {} sbom/ \;

# Live-instance integration tests. Reads ONMSCTL_TEST_URL / _USER /
# _PASSWORD; tests that don't see all three print "SKIP:" and return.
# Tests are #[ignore]d so `make test` is unaffected — only this target
# (and the matching CI job) exercises them.
#
# --test-threads=1 forces serial execution. Each test does a broad
# `onmsctl-it-*` cleanup before and after its own work; parallel tests
# would clobber each other's in-flight resources.
integration:  ## Run live-Horizon integration tests (needs ONMSCTL_TEST_URL/USER/PASSWORD)
	cargo test -p onmsctl-it -- --include-ignored --nocapture --test-threads=1

# Regenerate the committed JSON Schemas from each capability's Rust
# types. Atomic writes per file so a generator crash doesn't corrupt
# the artifact. The per-crate `schema_drift` integration tests fail CI
# if any schema falls behind its source types.
schema:  ## Regenerate every committed schemas/*.schema.json from the Rust types
	@mkdir -p schemas
	cargo run --quiet --release --example gen_schema -p onmsctl-eventconf \
		> schemas/event-source.schema.json.tmp \
		&& mv schemas/event-source.schema.json.tmp schemas/event-source.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-provisioning \
		> schemas/requisition.schema.json.tmp \
		&& mv schemas/requisition.schema.json.tmp schemas/requisition.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-iam \
		> schemas/iam-user.schema.json.tmp \
		&& mv schemas/iam-user.schema.json.tmp schemas/iam-user.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-snmp \
		> schemas/snmp-config.schema.json.tmp \
		&& mv schemas/snmp-config.schema.json.tmp schemas/snmp-config.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-maintenance \
		> schemas/maintenance.schema.json.tmp \
		&& mv schemas/maintenance.schema.json.tmp schemas/maintenance.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-datacollection \
		> schemas/datacollection.schema.json.tmp \
		&& mv schemas/datacollection.schema.json.tmp schemas/datacollection.schema.json
	cargo run --quiet --release --example gen_schema -p onmsctl-businessservice \
		> schemas/business-service.schema.json.tmp \
		&& mv schemas/business-service.schema.json.tmp schemas/business-service.schema.json
