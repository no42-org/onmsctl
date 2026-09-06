.PHONY: help build test verify fmt clippy deny fuzz-check fuzz lint-actions licenses licenses-check install-tools install-cargo-deny install-cargo-about install-cargo-cyclonedx install-cargo-fuzz install-actionlint install-zizmor tool-pin-hashes release-build sbom integration schema docker clean

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
	$(CARGO_DENY) check

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
	$(CARGO_DENY) --manifest-path fuzz/Cargo.toml check advisories licenses sources

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
	@test -f 'fuzz/fuzz_targets/$(FUZZ_TARGET).rs' || { \
	  echo "fuzz: unknown target '$(FUZZ_TARGET)'. Targets: $(basename $(notdir $(wildcard fuzz/fuzz_targets/*.rs)))" >&2; \
	  exit 1; \
	}
	@test -d 'fuzz/seeds/$(FUZZ_TARGET)' || { \
	  echo "fuzz: no seed directory fuzz/seeds/$(FUZZ_TARGET)/. Every target ships seeds; add at least one input there." >&2; \
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

install-tools: install-cargo-deny install-cargo-about install-cargo-cyclonedx install-cargo-fuzz install-actionlint install-zizmor

# Pinned: the licenses-drift gate diffs a fresh regeneration against the
# committed THIRD-PARTY-LICENSES.md, so every regeneration — local or CI —
# must come from the same cargo-about version, or formatting/content
# differences between releases of the tool read as license drift. This is
# why cargo-about, unlike the prebuilt tools below, reinstalls on a version
# mismatch instead of warning.
CARGO_ABOUT_VERSION ?= 0.9.1

install-cargo-about:
	@installed="$$(cargo about --version 2> /dev/null | awk '{print $$2}')"; \
	test "$$installed" = "$(CARGO_ABOUT_VERSION)" || \
	  cargo install --locked --features=cli cargo-about --version $(CARGO_ABOUT_VERSION)

# Not pinned: `make fuzz` is local-only and runs on a nightly toolchain that
# moves under it anyway, so a cargo-fuzz pin would buy nothing.
install-cargo-fuzz:
	@command -v cargo-fuzz > /dev/null 2>&1 || cargo install --locked cargo-fuzz

# ---- Pinned prebuilt tools ---------------------------------------------------
#
# cargo-deny, cargo-cyclonedx, actionlint and zizmor are fetched as pinned
# upstream release binaries into .bin/, not built with `cargo install`.
# Three reasons. A `cargo install` lands in ~/.cargo/bin, which rust-cache
# restores in CI, so the version a gate ran at was whatever was compiled
# when that cache entry was created, and it changed whenever an unrelated
# Cargo.lock change rolled the key (local cargo-deny 0.19.4 vs CI 0.20.2 is
# how #118 started). A prebuilt binary cannot fail to build against the
# pinned toolchain (zizmor already needs a newer rustc than we pin;
# actionlint is Go). And it keeps CI fast.
#
# Every archive is verified against the SHA-256 committed below for that
# tool and host before it is extracted. The hashes are ours, not upstream's
# checksum files: the four projects publish those in three formats or not
# at all, and they sit on the same release page as the archive. To bump a
# tool, edit its *_VERSION, run `make tool-pin-hashes TOOL=<name>
# VERSION=<ver>`, paste the printed block over the old one, and run the gate
# that uses the tool. Dependabot does not see these pins; CONTRIBUTING.md
# "Tool pins" has the procedure.
#
# A tool already on PATH wins and is never replaced. If its version differs
# from the pin the recipe says so once on stderr and continues. CI runners
# have none of these on PATH, so they always run the exact pin.
BIN_DIR := $(CURDIR)/.bin

UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)
ifeq ($(UNAME_S),Darwin)
  HOST_OS := apple-darwin
else
  HOST_OS := unknown-linux-gnu
endif
ifeq ($(UNAME_M),x86_64)
  HOST_ARCH := x86_64
else
  HOST_ARCH := aarch64
endif
# Key into the hash tables below; one of TOOL_HOSTS.
HOST := $(HOST_ARCH)-$(HOST_OS)
TOOL_HOSTS := aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu

# Release asset URL per tool: $(1) version, $(2) arch, $(3) os as a gnu
# triple suffix. Tools that ship musl on Linux or use Go-style names
# translate here, so the hash tables can all be keyed by the same HOST.
url_cargo-deny = https://github.com/EmbarkStudios/cargo-deny/releases/download/$(1)/cargo-deny-$(1)-$(2)-$(subst unknown-linux-gnu,unknown-linux-musl,$(3)).tar.gz
url_cargo-cyclonedx = https://github.com/CycloneDX/cyclonedx-rust-cargo/releases/download/cargo-cyclonedx-$(1)/cargo-cyclonedx-$(2)-$(3).tar.xz
url_zizmor = https://github.com/zizmorcore/zizmor/releases/download/v$(1)/zizmor-$(2)-$(3).tar.gz
url_actionlint = https://github.com/rhysd/actionlint/releases/download/v$(1)/actionlint_$(1)_$(if $(findstring darwin,$(3)),darwin,linux)_$(if $(findstring x86_64,$(2)),amd64,arm64).tar.gz

# How each tool reports its version number; $(1) is the binary path.
# cargo-cyclonedx only answers as the cargo subcommand it is.
version_cargo-deny = $(1) --version 2> /dev/null | awk '{print $$2}'
version_cargo-cyclonedx = $(1) cyclonedx --version 2> /dev/null | awk '{print $$2}'
version_zizmor = $(1) --version 2> /dev/null | awk '{print $$2}'
version_actionlint = $(1) --version 2> /dev/null | head -n 1 | sed 's/^v//'

CARGO_DENY_VERSION ?= 0.20.2
sha256_cargo-deny_aarch64-apple-darwin := fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3
sha256_cargo-deny_x86_64-apple-darwin := 248da7f581724e470071990c088ffc55c811981715f4cbdb258621fb79f8b7a6
sha256_cargo-deny_aarch64-unknown-linux-gnu := 995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d
sha256_cargo-deny_x86_64-unknown-linux-gnu := 9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

CARGO_CYCLONEDX_VERSION ?= 0.5.9
sha256_cargo-cyclonedx_aarch64-apple-darwin := 4c53dfa21e70b65bf7f8d2592aadde3bcb02c1a40b6ec63b877e5ca65a29e180
sha256_cargo-cyclonedx_x86_64-apple-darwin := 59d2a583fa632f8759456c1b531340331255b277386d23c598a3dbbc916fde63
sha256_cargo-cyclonedx_aarch64-unknown-linux-gnu := 7bf131ca5389b07a4f10c182bcf8a5ad339d64408b6f0d8f6834a0bd6120a06a
sha256_cargo-cyclonedx_x86_64-unknown-linux-gnu := fb8dbee9f182173e062a64a387b21a0badc6fab8b2abf9294973f012972bf6d8

ACTIONLINT_VERSION ?= 1.7.12
sha256_actionlint_aarch64-apple-darwin := aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f
sha256_actionlint_x86_64-apple-darwin := 5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644
sha256_actionlint_aarch64-unknown-linux-gnu := 325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6
sha256_actionlint_x86_64-unknown-linux-gnu := 8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8

ZIZMOR_VERSION ?= 1.28.0
sha256_zizmor_aarch64-apple-darwin := 54949bbd6b4c8527046bb8990bac9e0dab3eec787640f4e6199ae121dd1040be
sha256_zizmor_x86_64-apple-darwin := 40a58d8560d65c71357b3977d0da425773bf8f10bf1ffd38099d963d3afdf3aa
sha256_zizmor_aarch64-unknown-linux-gnu := 324e43770cfacf4216f8aefb287263b5b5c733c85b03bf7583b5cc4a0460239e
sha256_zizmor_x86_64-unknown-linux-gnu := e87b67160194884e375a46a12c57ccc904f762b53845f254fab7f17d98809c09

# PATH first, .bin/ second. Gate recipes call these instead of `cargo <tool>`
# so the resolved binary is the one that runs.
CARGO_DENY := $(shell command -v cargo-deny 2> /dev/null || echo $(BIN_DIR)/cargo-deny)
CARGO_CYCLONEDX := $(shell command -v cargo-cyclonedx 2> /dev/null || echo $(BIN_DIR)/cargo-cyclonedx)
ACTIONLINT := $(shell command -v actionlint 2> /dev/null || echo $(BIN_DIR)/actionlint)
ZIZMOR := $(shell command -v zizmor 2> /dev/null || echo $(BIN_DIR)/zizmor)

# $(call fetch_tool,<tool>,<version>): download the archive for HOST, verify
# it against the committed hash, extract the binary into .bin/. Any failure
# leaves nothing behind: the temp dir (and the archive in it) is removed.
define fetch_tool
set -e; \
url='$(call url_$(1),$(2),$(HOST_ARCH),$(HOST_OS))'; \
want='$(sha256_$(1)_$(HOST))'; \
test -n "$$want" || { echo "$(1): no committed SHA-256 for $(HOST). Run: make tool-pin-hashes TOOL=$(1) VERSION=$(2)" >&2; exit 1; }; \
mkdir -p '$(BIN_DIR)'; tmp="$$(mktemp -d)"; trap 'rm -rf "$$tmp"' EXIT; \
echo "fetching $(1) $(2) ($(HOST))"; \
curl -fsSL -o "$$tmp/archive" "$$url"; \
got="$$(shasum -a 256 "$$tmp/archive" | cut -c1-64)"; \
test "$$got" = "$$want" || { \
  echo "$(1) $(2) ($(HOST)): SHA-256 mismatch, not installing" >&2; \
  echo "  expected $$want" >&2; \
  echo "  got      $$got" >&2; \
  echo "  If you bumped $(1), refresh the hashes: make tool-pin-hashes TOOL=$(1) VERSION=$(2)" >&2; \
  exit 1; \
}; \
tar -xf "$$tmp/archive" -C "$$tmp"; \
bin="$$(find "$$tmp" -type f -name '$(1)' | head -n 1)"; \
test -n "$$bin" || { echo "$(1): archive contains no '$(1)' binary" >&2; exit 1; }; \
mv "$$bin" '$(BIN_DIR)/$(1)'; chmod +x '$(BIN_DIR)/$(1)'
endef

# $(call ensure_tool,<tool>,<version>,<resolved path>): PATH copy present ->
# warn on version mismatch, keep it; .bin/ copy missing or at another
# version -> fetch the pin.
define ensure_tool
if [ '$(3)' != '$(BIN_DIR)/$(1)' ]; then \
  v="$$($(call version_$(1),'$(3)'))"; \
  test "$$v" = '$(2)' || echo "warning: $(1) $$v found on PATH at $(3); CI pins $(2)" >&2; \
elif [ "$$($(call version_$(1),'$(3)'))" != '$(2)' ]; then \
  $(call fetch_tool,$(1),$(2)); \
fi
endef

install-cargo-deny:
	@$(call ensure_tool,cargo-deny,$(CARGO_DENY_VERSION),$(CARGO_DENY))

install-cargo-cyclonedx:
	@$(call ensure_tool,cargo-cyclonedx,$(CARGO_CYCLONEDX_VERSION),$(CARGO_CYCLONEDX))

install-actionlint:
	@$(call ensure_tool,actionlint,$(ACTIONLINT_VERSION),$(ACTIONLINT))

install-zizmor:
	@$(call ensure_tool,zizmor,$(ZIZMOR_VERSION),$(ZIZMOR))

# Bump helper: print the sha256_<tool>_<host> block for every host in
# TOOL_HOSTS. Downloads into a temp dir only; .bin/ is untouched.
tool-pin-hashes:  ## Print committed-hash lines for TOOL=<name> VERSION=<ver> (bump helper)
	@test -n "$(TOOL)" && test -n "$(VERSION)" || { echo "usage: make tool-pin-hashes TOOL=cargo-deny VERSION=0.20.2" >&2; exit 1; }
	@test -n "$(url_$(TOOL))" || { echo "tool-pin-hashes: unknown TOOL '$(TOOL)'. Known: cargo-deny cargo-cyclonedx actionlint zizmor" >&2; exit 1; }
	@set -e; tmp="$$(mktemp -d)"; trap 'rm -rf "$$tmp"' EXIT; \
	$(foreach h,$(TOOL_HOSTS),\
	  curl -fsSL -o "$$tmp/a" '$(call url_$(TOOL),$(VERSION),$(firstword $(subst -, ,$(h))),$(patsubst $(firstword $(subst -, ,$(h)))-%,%,$(h)))'; \
	  printf 'sha256_$(TOOL)_$(h) := %s\n' "$$(shasum -a 256 "$$tmp/a" | cut -c1-64)"; )

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
	$(CARGO_CYCLONEDX) cyclonedx --format json --spec-version 1.5
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
