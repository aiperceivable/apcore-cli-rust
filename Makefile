.PHONY: setup check test lint fmt clean build doc doc-clean coverage

# One-time dev environment setup
setup:
	@echo "Installing apdev-rs..."
	@command -v apdev-rs >/dev/null 2>&1 || cargo install apdev-rs
	@echo "Installing git pre-commit hook..."
	@mkdir -p .git/hooks
	@cp hooks/pre-commit .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "Done! Development environment is ready."

# Run all checks (same as pre-commit hook)
check: fmt-check lint check-chars test

check-chars:
	apdev-rs check-chars src/

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

# Build release binary and symlink to .bin/
build:
	cargo build --release
	@mkdir -p .bin
	@ln -sf ../target/release/apcore-cli .bin/apcore-cli
	@echo "Binary ready: .bin/apcore-cli"
	@echo "Usage: PATH=.bin:\$$PATH apcore-cli --extensions-dir examples/extensions list"

fmt:
	cargo fmt --all

# Regenerate rustdoc HTML. Run before publishing release artifacts so
# target/doc/ does not list removed symbols (D9-001).
doc:
	cargo doc --no-deps

# Wipe stale doc output and regenerate. Use this when symbols have been
# removed from the public surface (e.g., the v0.7.0 CliConfig removal).
doc-clean:
	cargo clean --doc
	cargo doc --no-deps

# Run tests under coverage instrumentation. Requires cargo-llvm-cov:
#   cargo install cargo-llvm-cov
# Outputs HTML report to target/llvm-cov/html/index.html.
coverage:
	cargo llvm-cov --html --workspace

clean:
	cargo clean
	@rm -rf .bin
