# Homebrew's rustup isn't on PATH by default, so look for cargo there too.
# (macOS make 3.81 ignores PATH changes for command lookup, hence the full path.)
CARGO ?= $(shell PATH="/opt/homebrew/opt/rustup/bin:$(HOME)/.cargo/bin:$$PATH" command -v cargo)
ifeq ($(CARGO),)
$(error cargo not found — install Rust: brew install rustup && rustup default stable)
endif
# Subcommands like clippy/fmt are resolved via PATH by cargo itself.
export PATH := $(dir $(CARGO)):$(PATH)

BIN := target/release/maximus
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
DEV_DIR := .dev

.PHONY: build release run dev test lint fmt check install uninstall clean help dist-plan tag

.DEFAULT_GOAL := help

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

build: ## Debug build
	$(CARGO) build

release: ## Optimized build (target/release/maximus)
	$(CARGO) build --release

run: ## Run the release build with your real config
	@$(CARGO) build --release --quiet
	@$(BIN) $(ARGS)

dev: ## Run a debug build with config/state isolated in .dev/
	@$(CARGO) build --quiet
	@MAXIMUS_CONFIG_DIR=$(DEV_DIR)/config MAXIMUS_STATE_DIR=$(DEV_DIR)/state target/debug/maximus $(ARGS)

test: ## Run unit tests
	$(CARGO) test

lint: ## Clippy, warnings as errors
	$(CARGO) clippy --all-targets -- -D warnings

fmt: ## Format the code
	$(CARGO) fmt

check: ## Formatting check + tests (CI-style)
	$(CARGO) fmt --check
	$(CARGO) test

install: ## Install to ~/.cargo/bin
	$(CARGO) install --path . --locked

uninstall: ## Remove from ~/.cargo/bin
	$(CARGO) uninstall maximus

clean: ## Remove build output and .dev/
	$(CARGO) clean
	rm -rf $(DEV_DIR)

dist-plan: ## Preview what a release would build and publish
	@dist plan

tag: ## Tag the Cargo.toml version and push it, triggering a GitHub release
	@git diff --quiet && git diff --cached --quiet || { echo "Commit your changes first."; exit 1; }
	@git rev-parse -q --verify "refs/tags/v$(VERSION)" >/dev/null && { echo "v$(VERSION) already exists; bump version in Cargo.toml."; exit 1; } || true
	git tag -a "v$(VERSION)" -m "v$(VERSION)"
	git push origin "v$(VERSION)"
