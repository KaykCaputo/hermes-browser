SHELL := /bin/sh

.DEFAULT_GOAL := help

CARGO ?= cargo
MKDOCS ?= mkdocs
PYTHON ?= python3
RUSTUP ?= rustup
STRIP ?= strip
APP_PACKAGE ?= mercury-desktop
ARGS ?=
RUST_LOG ?= talaria_servo=info,servo=warn
DIST_DIR ?= dist
PROJECT_VERSION ?= 0.2.0
LINUX_PACKAGE_NAME := mercury-browser-v$(PROJECT_VERSION)-linux-x86_64
LINUX_ARCHIVE := $(LINUX_PACKAGE_NAME).tar.gz

# macOS builds use a private, unpacked copy of the official GStreamer framework
# (scripts/macos/gstreamer.sh; `make gstreamer-macos` prepares it). Recipes that
# compile or run Servo load its environment first, the executables get their
# GStreamer libraries copied into target/<profile>/lib, and test binaries, which
# have no rpath to those libraries, find them through DYLD_FALLBACK_LIBRARY_PATH.
# That variable is set on the cargo command line itself because macOS strips
# DYLD_* variables when the SIP-protected make and /bin/sh start.
ifeq ($(shell uname -s),Darwin)
GSTREAMER_ENV = eval "$$(scripts/macos/gstreamer.sh env)" &&
COLLECT_GSTREAMER = scripts/macos/collect-gstreamer.sh "$$MERCURY_GSTREAMER_ROOT"
TEST_LIBRARY_PATH = DYLD_FALLBACK_LIBRARY_PATH="$$MERCURY_GSTREAMER_ROOT/lib"
else
GSTREAMER_ENV =
COLLECT_GSTREAMER =
TEST_LIBRARY_PATH =
endif

.PHONY: \
	help \
	doctor \
	setup \
	install-linux-deps \
	gstreamer-macos \
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
	run-debug \
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
		'gstreamer-macos' 'Download and unpack GStreamer for macOS (no sudo).' \
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
		'run' 'Run the optimized Mercury Desktop build.' \
		'run-debug' 'Run Mercury Desktop with development debug information.' \
		'run-release' 'Alias for the optimized run target.' \
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
	@printf '%s\n' 'Installing the same packages CI uses (.github/workflows/ci.yml). The' \
		'clang pinning follows the runner (clang-14/libclang-14-dev on Ubuntu 22.04);' \
		'releases that ship a different clang need the matching package names here.'
	sudo apt-get update
	sudo apt-get install --yes \
		build-essential \
		clang-14 \
		cmake \
		gstreamer1.0-libav \
		gstreamer1.0-plugins-bad \
		gstreamer1.0-plugins-base \
		gstreamer1.0-plugins-good \
		gstreamer1.0-tools \
		gstreamer1.0-plugins-ugly \
		gstreamer1.0-pulseaudio \
		libclang-14-dev \
		libfontconfig1-dev \
		libgstreamer1.0-dev \
		libgstreamer-plugins-base1.0-dev \
		libgstreamer-plugins-bad1.0-dev \
		libunwind-dev \
		libx11-dev \
		libx11-xcb-dev \
		libxcursor-dev \
		libxkbcommon-x11-dev \
		ninja-build \
		pkg-config \
		xinput

gstreamer-macos:
	scripts/macos/gstreamer.sh fetch
	scripts/macos/gstreamer.sh unpack
	scripts/macos/gstreamer.sh check

fetch:
	"$(CARGO)" fetch --locked

check:
	$(GSTREAMER_ENV) "$(CARGO)" check --workspace --all-targets --all-features

fmt:
	"$(CARGO)" fmt --all

fmt-check:
	"$(CARGO)" fmt --all -- --check

lint:
	$(GSTREAMER_ENV) "$(CARGO)" clippy \
		--workspace \
		--all-targets \
		--all-features \
		-- \
		-D warnings

test:
	$(GSTREAMER_ENV) $(TEST_LIBRARY_PATH) "$(CARGO)" test --workspace
	"$(CARGO)" test --target-dir target --manifest-path third_party/gaol/Cargo.toml --test shared-memory

qa:
	"$(MAKE)" fmt-check
	"$(MAKE)" lint
	"$(MAKE)" test

build:
	$(GSTREAMER_ENV) "$(CARGO)" build --workspace
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) $(COLLECT_GSTREAMER) target/debug/lib)

build-release:
	$(GSTREAMER_ENV) "$(CARGO)" build --workspace --release
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) $(COLLECT_GSTREAMER) target/release/lib)

package-linux: build-release
	mkdir -p "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)"
	cp "target/release/mercury-browser" "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/mercury-browser"
	"$(STRIP)" --strip-unneeded "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/mercury-browser"
	cp README.md "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/README.md"
	cp LICENSE "$(DIST_DIR)/$(LINUX_PACKAGE_NAME)/LICENSE"
	tar -C "$(DIST_DIR)" -czf "$(DIST_DIR)/$(LINUX_ARCHIVE)" "$(LINUX_PACKAGE_NAME)"
	cd "$(DIST_DIR)" && sha256sum "$(LINUX_ARCHIVE)" > "$(LINUX_ARCHIVE).sha256"

run:
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) "$(CARGO)" build --package "$(APP_PACKAGE)" --release)
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) $(COLLECT_GSTREAMER) target/release/lib)
	$(GSTREAMER_ENV) RUST_LOG="$(RUST_LOG)" "$(CARGO)" run --package "$(APP_PACKAGE)" --release -- $(ARGS)

run-debug:
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) "$(CARGO)" build --package "$(APP_PACKAGE)")
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) $(COLLECT_GSTREAMER) target/debug/lib)
	$(GSTREAMER_ENV) RUST_LOG="$(RUST_LOG)" "$(CARGO)" run --package "$(APP_PACKAGE)" -- $(ARGS)

run-release: run

docs-install:
	"$(PYTHON)" -m pip install -r docs/requirements.txt

docs:
	"$(MKDOCS)" build --strict

docs-serve:
	"$(MKDOCS)" serve

rust-docs:
	$(GSTREAMER_ENV) "$(CARGO)" doc --workspace --no-deps

rust-docs-open:
	$(GSTREAMER_ENV) "$(CARGO)" doc --workspace --no-deps --open

tree:
	"$(CARGO)" tree --workspace

clean:
	"$(CARGO)" clean
