.PHONY: help build test verify fmt clippy deny licenses install-tools install-cargo-deny install-cargo-about install-cargo-cyclonedx release-build sbom integration schema clean

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

licenses: install-cargo-about  ## Regenerate THIRD-PARTY-LICENSES.md from the dep tree
	# Atomic write: failure leaves the existing file untouched.
	cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.md.tmp about.hbs && \
		mv THIRD-PARTY-LICENSES.md.tmp THIRD-PARTY-LICENSES.md

clean:  ## Remove the cargo target directory
	cargo clean

install-tools: install-cargo-deny install-cargo-about

install-cargo-deny:
	@command -v cargo-deny > /dev/null 2>&1 || cargo install --locked cargo-deny

install-cargo-about:
	@command -v cargo-about > /dev/null 2>&1 || cargo install --locked --features=cli cargo-about

install-cargo-cyclonedx:
	@command -v cargo-cyclonedx > /dev/null 2>&1 || cargo install --locked cargo-cyclonedx

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
