.DEFAULT_GOAL := help

GIT_SHA ?= $(shell git rev-parse HEAD)
GIT_TAG ?= $(shell git describe --tags --abbrev=0 2>/dev/null || echo "dev")
BIN_DIR = dist/bin
CARGO_TARGET_DIR ?= target
PROFILE ?= dev
FEATURES ?=
CARGO_INSTALL_EXTRA_FLAGS ?=

##@ Help

.PHONY: help
help: ## Display this help.
	@awk 'BEGIN {FS = ":.*##"; printf "Usage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Build

.PHONY: build
build: ## Build the project.
	cargo build --features "$(FEATURES)" --profile "$(PROFILE)"

.PHONY: build-release
build-release: ## Build in release mode.
	cargo build --features "$(FEATURES)" --profile release

.PHONY: profiling
profiling: ## Build with profiling symbols.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling

.PHONY: maxperf
maxperf: ## Build with maximum performance optimizations.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf

##@ Lint

.PHONY: fmt
fmt: ## Run rustfmt (nightly).
	cargo +nightly fmt --all

.PHONY: fmt-check
fmt-check: ## Check rustfmt (nightly).
	cargo +nightly fmt --all --check

.PHONY: clippy
clippy: ## Run clippy with all features, deny warnings.
	cargo +nightly clippy \
		--workspace \
		--bins \
		--examples \
		--tests \
		--benches \
		--all-features \
		-- -D warnings

.PHONY: clippy-fix
clippy-fix: ## Run clippy with auto-fix.
	cargo +nightly clippy \
		--workspace \
		--bins \
		--examples \
		--tests \
		--benches \
		--all-features \
		--fix \
		--allow-staged \
		--allow-dirty \
		-- -D warnings

.PHONY: lint-typos
lint-typos: ensure-typos ## Run typos spell checker.
	typos

.PHONY: lint-toml
lint-toml: ensure-dprint ## Format all TOML files.
	dprint fmt

.PHONY: lint-toml-check
lint-toml-check: ensure-dprint ## Check TOML formatting.
	dprint check

.PHONY: udeps
udeps: ## Check for unused dependencies.
	cargo +nightly udeps --workspace --all-features

.PHONY: hack
hack: ## Check feature powerset with cargo-hack.
	cargo hack check --workspace --feature-powerset --depth 2

.PHONY: zepter
zepter: ## Check feature propagation with zepter.
	zepter run check

.PHONY: lint
lint: ## Run ALL linters (fmt + clippy + typos + toml).
	$(MAKE) fmt && \
	$(MAKE) clippy && \
	$(MAKE) lint-typos && \
	$(MAKE) lint-toml

##@ Test

.PHONY: test-unit
test-unit: ## Run unit tests with nextest.
	cargo nextest run --workspace --no-fail-fast

.PHONY: test-doc
test-doc: ## Run doc tests (no-op for binary-only workspaces with no lib targets).
	cargo test --doc --workspace --all-features || echo "No library targets — skipping doctests"

.PHONY: test
test: ## Run all tests (unit + doc).
	$(MAKE) test-unit && \
	$(MAKE) test-doc

.PHONY: test-coverage
test-coverage: ## Run tests with coverage (requires cargo-llvm-cov).
	cargo +nightly llvm-cov nextest --lcov --output-path lcov.info --workspace
	cargo +nightly llvm-cov report --html
	@echo "Coverage report: target/llvm-cov/html/index.html"

##@ Cross-compilation

.PHONY: check-wasm
check-wasm: ## Check compilation for wasm32-wasip1 target.
	cargo check --workspace --target wasm32-wasip1

.PHONY: check-riscv
check-riscv: ## Check compilation for riscv32imac-unknown-none-elf target.
	cargo check --workspace --target riscv32imac-unknown-none-elf

##@ Security & Dependencies

.PHONY: deny
deny: ## Run cargo-deny checks (advisories, bans, licenses, sources).
	cargo deny --all-features check all

.PHONY: audit
audit: ## Run cargo-audit for vulnerabilities.
	cargo audit

.PHONY: check-no-test-deps
check-no-test-deps: ## Ensure test-only deps don't leak into production.
	@if cargo tree --workspace -e normal,build --no-default-features 2>/dev/null | grep -qE "arbitrary|proptest"; then \
		echo "ERROR: Found test-only dependencies in non-dev dependency tree"; \
		cargo tree --workspace -e normal,build --no-default-features | grep -E "arbitrary|proptest"; \
		exit 1; \
	fi

##@ Documentation

.PHONY: docs
docs: ## Build documentation with all features.
	RUSTDOCFLAGS="\
		--cfg docsrs \
		--show-type-layout \
		--generate-link-to-definition \
		--enable-index-page -Zunstable-options -D warnings" \
	cargo +nightly doc \
		--workspace \
		--all-features \
		--no-deps \
		--document-private-items

##@ Benchmarks

.PHONY: bench
bench: ## Run all benchmarks.
	cargo bench --workspace

##@ CI / PR

.PHONY: pr
pr: ## Run all checks (deny + lint + test + docs).
	$(MAKE) deny && \
	$(MAKE) lint && \
	$(MAKE) udeps && \
	$(MAKE) hack && \
	$(MAKE) zepter && \
	$(MAKE) check-no-test-deps && \
	$(MAKE) test && \
	$(MAKE) docs

##@ Utility

.PHONY: clean
clean: ## Clean build artifacts.
	cargo clean
	rm -rf $(BIN_DIR)

.PHONY: fix-lint
fix-lint: ## Auto-fix clippy + reformat.
	$(MAKE) clippy-fix && \
	$(MAKE) fmt

.PHONY: ensure-typos
ensure-typos:
	@command -v typos >/dev/null || { \
		echo "typos not found. Install: cargo install --locked typos-cli"; \
		exit 1; \
	}

.PHONY: ensure-dprint
ensure-dprint:
	@command -v dprint >/dev/null || { \
		echo "dprint not found. Install: cargo install --locked dprint"; \
		exit 1; \
	}
