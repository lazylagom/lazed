BUN ?= bun
CARGO ?= cargo

.PHONY: help install dev web fetch-herdr check test format build dist schema-check perf clean

help: ## show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  make %-14s %s\n", $$1, $$2}'

install: ## install frontend deps
	$(BUN) install

dev: ## run the app (tauri dev — starts vite + native shell)
	$(BUN) run tauri dev

web: ## frontend only (vite dev server on :1420)
	$(BUN) run dev

fetch-herdr: ## stage the herdr binary into src-tauri/bin
	$(BUN) run fetch-herdr

check: ## typecheck + lint + cargo check
	$(BUN) run check
	$(CARGO) check --manifest-path src-tauri/Cargo.toml

test: ## vitest + cargo test
	$(BUN) run test
	$(CARGO) test --manifest-path src-tauri/Cargo.toml

format: ## biome format --write
	$(BUN) run format

build: ## frontend production build (dist/)
	$(BUN) run build

dist: ## full app bundle (fetch-herdr + tauri build)
	$(BUN) run dist

schema-check: ## verify herdr schema fields staylazy depends on
	python3 scripts/check_herdr_schema.py

perf: ## frame-stream perf measurement (throwaway herdr session)
	python3 scripts/perf_stream.py

clean: ## remove generated artifacts (dist/, bundled herdr, cargo target)
	rm -rf dist src-tauri/bin/herdr
	$(CARGO) clean --manifest-path src-tauri/Cargo.toml
