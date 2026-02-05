SHELL := /bin/bash
.DEFAULT_GOAL := help

.PHONY: help setup setup-env setup-sidecar setup-rust run run-sidecar run-api test

help:
	@echo "Targets:"
	@echo "  make setup       - prepare local dev environment"
	@echo "  make run         - start sidecar + Rust API together"
	@echo "  make run-sidecar - start Docling sidecar only"
	@echo "  make run-api     - start Rust API only"
	@echo "  make test        - run Rust tests"

setup: setup-env setup-sidecar setup-rust
	@echo "Setup complete."

setup-env:
	@if [ ! -f .env ]; then \
		cp .env.example .env; \
		echo "Created .env from .env.example. Set OPENROUTER_API_KEY before running."; \
	else \
		echo ".env already exists."; \
	fi

setup-sidecar:
	@command -v uv >/dev/null || (echo "uv is required: https://docs.astral.sh/uv/" && exit 1)
	uv sync --project docling-sidecar

setup-rust:
	@command -v cargo >/dev/null || (echo "cargo is required: https://www.rust-lang.org/tools/install" && exit 1)
	cargo fetch

run:
	./scripts/dev.sh

run-sidecar:
	@command -v uv >/dev/null || (echo "uv is required: https://docs.astral.sh/uv/" && exit 1)
	uv run --project docling-sidecar uvicorn server:app --app-dir docling-sidecar --host 0.0.0.0 --port 3001

run-api:
	@command -v cargo >/dev/null || (echo "cargo is required: https://www.rust-lang.org/tools/install" && exit 1)
	cargo run

test:
	cargo test
