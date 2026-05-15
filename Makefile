.PHONY: build test verify fmt clippy deny licenses install-tools install-cargo-deny install-cargo-about install-cargo-cyclonedx release-build sbom integration schema clean

build:
	cargo build --workspace --all-targets

test:
	cargo test --workspace --all-targets

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

deny: install-cargo-deny
	cargo deny check

verify: fmt clippy build test deny

licenses: install-cargo-about
	# Atomic write: failure leaves the existing file untouched.
	cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.md.tmp about.hbs && \
		mv THIRD-PARTY-LICENSES.md.tmp THIRD-PARTY-LICENSES.md

clean:
	cargo clean

install-tools: install-cargo-deny install-cargo-about

install-cargo-deny:
	@command -v cargo-deny > /dev/null 2>&1 || cargo install --locked cargo-deny

install-cargo-about:
	@command -v cargo-about > /dev/null 2>&1 || cargo install --locked cargo-about

install-cargo-cyclonedx:
	@command -v cargo-cyclonedx > /dev/null 2>&1 || cargo install --locked cargo-cyclonedx

# Release-only targets. CI invokes these from .github/workflows/release.yml
# so the local developer command and the CI command stay in sync.

# Build a stripped, optimized binary for $(TARGET). The release workflow
# adds the target via `rustup target add` before invoking us.
release-build:
	@test -n "$(TARGET)" || (echo "release-build: TARGET=<triple> required" >&2; exit 1)
	cargo build --release --bin onmsctl --target $(TARGET)

# Generate per-crate CycloneDX SBOMs at sbom/<crate>.cdx.json.
#
# cargo-cyclonedx writes one `<crate>.cdx.json` next to each member's
# Cargo.toml. Default filenames already disambiguate — `onmsctl.cdx.json`,
# `onmsctl-core.cdx.json`, `onmsctl-eventconf.cdx.json`. The previous
# `--override-filename onmsctl` flag forced every crate to write to the
# same name, causing the follow-up `mv` to clobber two of three SBOMs.
sbom: install-cargo-cyclonedx
	@mkdir -p sbom
	cargo cyclonedx --format json
	@find . -maxdepth 3 -name '*.cdx.json' -not -path './sbom/*' -exec mv {} sbom/ \;

# Live-instance integration tests. Reads ONMSCTL_TEST_URL / _USER /
# _PASSWORD; tests that don't see all three print "SKIP:" and return.
# Tests are #[ignore]d so `make test` is unaffected — only this target
# (and the matching CI job) exercises them.
integration:
	cargo test -p onmsctl-it -- --include-ignored --nocapture

# Regenerate schemas/event-source.schema.json from EventSourceLocal.
# Atomic write so a generator crash doesn't corrupt the file. The
# `schema_matches_committed` test fails CI if this drifts from the
# type definitions.
schema:
	@mkdir -p schemas
	cargo run --quiet --release --example gen_schema -p onmsctl-eventconf \
		> schemas/event-source.schema.json.tmp \
		&& mv schemas/event-source.schema.json.tmp schemas/event-source.schema.json
