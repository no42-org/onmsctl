.PHONY: build test verify fmt clippy deny licenses install-tools install-cargo-deny install-cargo-about clean

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
	cargo about generate -c about.toml about.hbs > THIRD-PARTY-LICENSES.md

clean:
	cargo clean

install-tools: install-cargo-deny install-cargo-about

install-cargo-deny:
	@command -v cargo-deny > /dev/null 2>&1 || cargo install --locked cargo-deny

install-cargo-about:
	@command -v cargo-about > /dev/null 2>&1 || cargo install --locked cargo-about
