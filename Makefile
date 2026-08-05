SHELL := /bin/sh

.DEFAULT_GOAL := help

CARGO ?= cargo
MKDOCS ?= mkdocs
PYTHON ?= python3
RUSTUP ?= rustup
STRIP ?= strip
APP_PACKAGE ?= hermes-desktop
ARGS ?=
RUST_LOG ?= talaria_servo=info,servo=warn
DIST_DIR ?= dist
PROJECT_VERSION ?= 0.1.1
LINUX_PACKAGE_NAME := hermes-browser-v$(PROJECT_VERSION)-linux-x86_64
LINUX_ARCHIVE := $(LINUX_PACKAGE_NAME).tar.gz

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
	package-linux \
	run \
	run-release \
	docs-install \
	docs \
	docs-serve \
	rust-docs \
	rust-docs-open \
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
		'package-linux' 'Build and package the Linux x86-64 release.' \
		'run' 'Run Hermes Desktop in debug mode.' \
		'run-release' 'Run Hermes Desktop in release mode.' \
		'docs-install' 'Install the MkDocs documentation dependencies.' \
		'docs' 'Build the MkDocs site with strict validation.' \
		'docs-serve' 'Serve the MkDocs site with live reload.' \
		'rust-docs' 'Build the Rust API documentation.' \
		'rust-docs-open' 'Build and open the Rust API documentation.' \
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
		libgstreamer1.0-dev \
		libgstreamer-plugins-base1.0-dev \
		libgstreamer-plugins-bad1.0-dev \
		libx11-dev \
		libx11-xcb-dev \
		libxcursor-dev \
		libxkbcommon-x11-dev \
		gstreamer1.0-libav \
		gstreamer1.0-plugins-bad \
		gstreamer1.0-plugins-base \
		gstreamer1.0-plugins-good \
		gstreamer1.0-tools \
		gstreamer1.0-plugins-ugly \
		gstreamer1.0-pulseaudio \
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
	"$(CARGO)" test --target-dir target --manifest-path third_party/gaol/Cargo.toml --test shared-memory
	"$(CARGO)" test --package servo-media-gstreamer --lib

qa:
	"$(MAKE)" fmt-check
	"$(MAKE)" lint
	"$(MAKE)" test

build:
	"$(CARGO)" build --workspace

build-release:
	"$(CARGO)" build --workspace --release

package-linux: build-release
	mkdir -p "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)"
	cp "target/release/$(APP_PACKAGE)" "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/hermes-browser"
	"$(STRIP)" --strip-unneeded "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/hermes-browser"
	cp README.md "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/README.md"
	cp LICENSE "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/LICENSE"
	tar -C "$(DIST_DIR)" -czf "$(DIST_DIR)/$(LINUX_ARCHIVE)" "$(LINUX_PACKAGE_NAME)"
	cd "$(DIST_DIR)" && sha256sum "$(LINUX_ARCHIVE)" > "$(LINUX_ARCHIVE).sha256"

run:
	RUST_LOG="$(RUST_LOG)" "$(CARGO)" run --package "$(APP_PACKAGE)" -- $(ARGS)

run-release:
	RUST_LOG="$(RUST_LOG)" "$(CARGO)" run --package "$(APP_PACKAGE)" --release -- $(ARGS)

docs-install:
	"$(PYTHON)" -m pip install -r docs/requirements.txt

docs:
	"$(MKDOCS)" build --strict

docs-serve:
	"$(MKDOCS)" serve

rust-docs:
	"$(CARGO)" doc --workspace --no-deps

rust-docs-open:
	"$(CARGO)" doc --workspace --no-deps --open

tree:
	"$(CARGO)" tree --workspace

clean:
	"$(CARGO)" clean
