BUN ?= bun
CARGO ?= cargo

.PHONY: help deps install uninstall dev web fetch-herdr check test format build dist schema-check perf clean

help: ## show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  make %-14s %s\n", $$1, $$2}'

deps: ## install frontend deps
	$(BUN) install

install: ## link lazed CLI + agent skills into ~/ (builds release daemon first)
	$(CARGO) build --release --manifest-path daemon/Cargo.toml
	daemon/target/release/lazed install

uninstall: ## remove lazed links; PURGE=1 also wipes state/config/worktrees
	@bin=$$(command -v lazed || echo daemon/target/release/lazed); \
	"$$bin" uninstall $(if $(PURGE),--purge) $(if $(YES),--yes)

dev: ## run the app (tauri dev — starts vite + native shell)
	$(BUN) run tauri dev

web: ## frontend only (vite dev server on :1420)
	$(BUN) run dev

fetch-herdr: ## stage the herdr binary into src-tauri/bin
	$(BUN) run fetch-herdr

check: ## typecheck + lint + cargo check
	$(BUN) run check
	$(CARGO) check --manifest-path src-tauri/Cargo.toml
	$(CARGO) check --manifest-path daemon/Cargo.toml

test: ## vitest + cargo test
	$(BUN) run test
	$(CARGO) test --manifest-path src-tauri/Cargo.toml
	$(CARGO) test --manifest-path daemon/Cargo.toml
	$(CARGO) build --manifest-path daemon/Cargo.toml
	python3 scripts/test_task_runtime.py

format: ## biome format --write
	$(BUN) run format

build: ## frontend production build (dist/)
	$(BUN) run build

dist: ## full app bundle (daemon + skills staged, then tauri build)
	$(BUN) run dist

schema-check: ## verify herdr schema fields lazed depends on
	python3 scripts/check_herdr_schema.py

perf: ## frame-stream perf measurement (throwaway herdr session)
	python3 scripts/perf_stream.py

clean: ## remove generated artifacts (dist/, staged bundle payload, cargo target)
	rm -rf dist src-tauri/bin src-tauri/skills
	$(CARGO) clean --manifest-path src-tauri/Cargo.toml
