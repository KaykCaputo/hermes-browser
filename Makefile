SHELL := /bin/sh

.DEFAULT_GOAL := help

CARGO ?= cargo
RUSTUP ?= rustup
APP_PACKAGE ?= hermes-desktop
ARGS ?=

.PHONY: \
	help \
	doctor \
	setup \
	install-linux-deps \
	fetch \
	check \
	fmt \
	fmt-check \
	lint \
	test \
	qa \
	build \
	build-release \
	run \
	run-release \
	docs \
	docs-open \
	tree \
	clean

help:
	@printf '%-22s %s\n' \
		'help' 'Show the available commands.' \
		'doctor' 'Check the required development tools.' \
		'setup' 'Install Rust components and fetch dependencies.' \
		'install-linux-deps' 'Install Linux dependencies for the current Slint backend.' \
		'fetch' 'Download Cargo dependencies without building.' \
		'check' 'Type-check every workspace target and feature.' \
		'fmt' 'Format all Rust source files.' \
		'fmt-check' 'Check Rust formatting without changing files.' \
		'lint' 'Run Clippy and reject every warning.' \
		'test' 'Run all workspace tests.' \
		'qa' 'Run formatting, linting, and tests.' \
		'build' 'Build the complete workspace in debug mode.' \
		'build-release' 'Build the complete workspace in release mode.' \
		'run' 'Run Hermes Desktop in debug mode.' \
		'run-release' 'Run Hermes Desktop in release mode.' \
		'docs' 'Build the workspace documentation.' \
		'docs-open' 'Build and open the workspace documentation.' \
		'tree' 'Display the dependency tree.' \
		'clean' 'Remove Cargo build artifacts.'

doctor:
	@command -v "$(CARGO)" >/dev/null 2>&1 || { \
		printf '%s\n' 'error: Cargo was not found.' >&2; \
		exit 1; \
	}
	@command -v "$(RUSTUP)" >/dev/null 2>&1 || { \
		printf '%s\n' 'error: rustup was not found.' >&2; \
		exit 1; \
	}
	@command -v rustc >/dev/null 2>&1 || { \
		printf '%s\n' 'error: rustc was not found.' >&2; \
		exit 1; \
	}
	@printf '%s\n' 'Development tools:'
	@rustc --version
	@"$(CARGO)" --version
	@"$(RUSTUP)" show active-toolchain

setup: doctor
	"$(RUSTUP)" component add rustfmt clippy
	"$(CARGO)" fetch --locked

install-linux-deps:
	@command -v apt-get >/dev/null 2>&1 || { \
		printf '%s\n' 'error: this target currently supports apt-based distributions only.' >&2; \
		exit 1; \
	}
	sudo apt-get update
	sudo apt-get install --yes \
		build-essential \
		pkg-config \
		libfontconfig1-dev \
		libx11-dev \
		libx11-xcb-dev \
		libxcursor-dev \
		libxkbcommon-x11-dev \
		xinput

fetch:
	"$(CARGO)" fetch --locked

check:
	"$(CARGO)" check --workspace --all-targets --all-features

fmt:
	"$(CARGO)" fmt --all

fmt-check:
	"$(CARGO)" fmt --all -- --check

lint:
	"$(CARGO)" clippy \
		--workspace \
		--all-targets \
		--all-features \
		-- \
		-D warnings

test:
	"$(CARGO)" test --workspace

qa:
	"$(MAKE)" fmt-check
	"$(MAKE)" lint
	"$(MAKE)" test

build:
	"$(CARGO)" build --workspace

build-release:
	"$(CARGO)" build --workspace --release

run:
	"$(CARGO)" run --package "$(APP_PACKAGE)" -- $(ARGS)

run-release:
	"$(CARGO)" run --package "$(APP_PACKAGE)" --release -- $(ARGS)

docs:
	"$(CARGO)" doc --workspace --no-deps

docs-open:
	"$(CARGO)" doc --workspace --no-deps --open

tree:
	"$(CARGO)" tree --workspace

clean:
	"$(CARGO)" clean
