# agentskillpack — developer tasks.
#
# Common targets: build, test, lint (clippy + fmt), demo, install, docker, clean.

CARGO ?= cargo
BIN   := agentskillpack

.PHONY: all build release test lint clippy fmt fmt-check demo demo-sh demo-ps \
        install docker clean help

all: build

## build: debug build of the library and CLI
build:
	$(CARGO) build

## release: optimized release build
release:
	$(CARGO) build --release

## test: run the full test suite
test:
	$(CARGO) test

## lint: clippy (deny warnings) + rustfmt check
lint: clippy fmt-check

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

## demo: run the bash demo suite (see demo-ps for PowerShell)
demo: demo-sh

demo-sh: build
	bash demos/run_all.sh

demo-ps: build
	pwsh demos/run_all.ps1

## install: install the CLI via cargo
install:
	$(CARGO) install --path .

## docker: build the container image
docker:
	docker build -t $(BIN) .

## clean: remove build artifacts
clean:
	$(CARGO) clean

## help: list targets
help:
	@grep -E '^## ' $(MAKEFILE_LIST) | sed 's/^## /  /'
