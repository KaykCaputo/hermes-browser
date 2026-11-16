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
CI_TOOLS_DIR ?= target/ci-tools
# The pinned CI tools from `make ci-tools` are found before any other copy.
CI_TOOLS_PATH = PATH="$(abspath $(CI_TOOLS_DIR))/bin:$$PATH"

# macOS builds use a private, unpacked copy of the official GStreamer framework
# (scripts/macos/gstreamer.sh; `make gstreamer-macos` prepares it). Recipes that
# compile or run Servo load its environment first; the output of `gstreamer.sh env`
# is captured before it is evaluated, so a missing or broken prefix stops the recipe
# instead of letting pkg-config find another GStreamer. Executables get their
# GStreamer libraries copied into target/<profile>/lib, and test binaries, which
# have no rpath to those libraries, find that directory through
# DYLD_FALLBACK_LIBRARY_PATH. It is set on the cargo command line itself because
# macOS strips DYLD_* variables when the SIP-protected make and /bin/sh start.
# CARGO_FLAGS adds flags to check, lint and test (for example --release).
CARGO_FLAGS ?=
PROFILE_DIR = $(if $(filter --release,$(CARGO_FLAGS)),release,debug)
ifeq ($(shell uname -s),Darwin)
GSTREAMER_ENV = gst_env="$$(scripts/macos/gstreamer.sh env)" && eval "$$gst_env" &&
COLLECT_GSTREAMER = scripts/macos/collect-gstreamer.sh "$$MERCURY_GSTREAMER_PREFIX"
TEST_LIBRARY_PATH = DYLD_FALLBACK_LIBRARY_PATH="$(CURDIR)/target/$(PROFILE_DIR)/lib"
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
	test-media-macos \
	fetch \
	check \
	fmt \
	fmt-check \
	lint \
	test \
	qa \
	qa-fast \
	ci-tools \
	lint-scripts \
	deny \
	build \
	build-release \
	package-linux \
	app-macos \
	package-macos \
	test-bundle-macos \
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
		'test-media-macos' 'Check the macOS GStreamer scripts (needs gstreamer-macos for all checks).' \
		'fetch' 'Download Cargo dependencies without building.' \
		'check' 'Type-check every workspace target and feature.' \
		'fmt' 'Format all Rust source files.' \
		'fmt-check' 'Check Rust formatting without changing files.' \
		'lint' 'Run Clippy and reject every warning.' \
		'test' 'Run all workspace tests.' \
		'qa' 'Run formatting, linting, and tests.' \
		'qa-fast' 'Check formatting, lint and test the crates that need no engine.' \
		'ci-tools' 'Download the pinned shellcheck, actionlint and cargo-deny.' \
		'lint-scripts' 'Lint scripts/ (shellcheck, py_compile) and the workflows (actionlint).' \
		'deny' 'Check dependencies against the policy in deny.toml.' \
		'build' 'Build the complete workspace in debug mode.' \
		'build-release' 'Build the complete workspace in release mode.' \
		'package-linux' 'Build and package the Linux x86-64 release.' \
		'app-macos' 'Build the release and assemble the signed Mercury Browser.app.' \
		'package-macos' 'Build Mercury Browser.app and its .dmg for macOS arm64.' \
		'test-bundle-macos' 'Check the macOS bundle script helpers (no build needed).' \
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
	@if [ "$$(uname -s)" = Darwin ]; then \
		scripts/macos/gstreamer.sh check || { \
			printf '%s\n' 'error: GStreamer for macOS is not set up; run: make gstreamer-macos' >&2; \
			exit 1; \
		}; \
	fi

# On macOS GStreamer is fetched first: doctor checks it, and nothing builds without it.
ifeq ($(shell uname -s),Darwin)
SETUP_FIRST = gstreamer-macos
else
SETUP_FIRST =
endif

setup: $(SETUP_FIRST) doctor
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

test-media-macos:
	scripts/macos/test-media.sh

fetch:
	"$(CARGO)" fetch --locked

check:
	$(GSTREAMER_ENV) "$(CARGO)" check $(CARGO_FLAGS) --workspace --all-targets --all-features

fmt:
	"$(CARGO)" fmt --all

fmt-check:
	"$(CARGO)" fmt --all -- --check

lint:
	$(GSTREAMER_ENV) "$(CARGO)" clippy $(CARGO_FLAGS) \
		--workspace \
		--all-targets \
		--all-features \
		-- \
		-D warnings

test:
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) "$(CARGO)" build $(CARGO_FLAGS) --package "$(APP_PACKAGE)")
	$(if $(COLLECT_GSTREAMER),$(GSTREAMER_ENV) $(COLLECT_GSTREAMER) target/$(PROFILE_DIR)/lib)
	$(GSTREAMER_ENV) $(TEST_LIBRARY_PATH) "$(CARGO)" test $(CARGO_FLAGS) --workspace
	"$(CARGO)" test --target-dir target --manifest-path third_party/gaol/Cargo.toml --test shared-memory

qa:
	"$(MAKE)" fmt-check
	"$(MAKE)" lint
	"$(MAKE)" test

qa-fast:
	"$(MAKE)" fmt-check
	@packages="$$(scripts/ci/light-crates.sh)" || exit 1; \
	printf 'engine-free crates: %s\n' "$$packages"; \
	"$(CARGO)" clippy --locked --release $$packages --all-targets -- -D warnings && \
	"$(CARGO)" test --locked --release $$packages

ci-tools:
	scripts/ci/install-tools.sh --dir "$(CI_TOOLS_DIR)"

lint-scripts:
	@$(CI_TOOLS_PATH); export PATH; \
	missing=; \
	for tool in shellcheck actionlint; do \
		command -v "$$tool" >/dev/null 2>&1 || missing="$$missing $$tool"; \
	done; \
	if [ -n "$$missing" ]; then \
		printf 'error: not found:%s\n' "$$missing" >&2; \
		printf '%s\n' \
			'Run "make ci-tools" to download the pinned versions into $(CI_TOOLS_DIR)/bin' \
			'(their checksums are verified), or install shellcheck and actionlint yourself' \
			'at the versions in scripts/ci/install-tools.sh.' >&2; \
		exit 1; \
	fi; \
	printf 'shellcheck %s (%s), actionlint %s (%s)\n' \
		"$$(shellcheck --version | sed -n 's/^version: //p')" "$$(command -v shellcheck)" \
		"$$(actionlint -version | sed -n 1p)" "$$(command -v actionlint)"; \
	find scripts -type f -name '*.sh' -print0 | xargs -0 shellcheck -x && \
	find scripts -type f -name '*.py' -print0 | \
		PYTHONPYCACHEPREFIX="$(abspath target)/pycache" xargs -0 "$(PYTHON)" -m py_compile && \
	actionlint && \
	printf '%s\n' 'scripts and workflows: no findings'

deny:
	@$(CI_TOOLS_PATH); export PATH; \
	if ! command -v cargo-deny >/dev/null 2>&1; then \
		printf '%s\n' \
			'error: cargo-deny not found.' \
			'Run "make ci-tools" to download the pinned version into $(CI_TOOLS_DIR)/bin' \
			'(its checksum is verified), or install cargo-deny yourself at the version in' \
			'scripts/ci/install-tools.sh.' >&2; \
		exit 1; \
	fi; \
	cargo-deny --locked check --show-stats

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

REQUIRE_MACOS = @[ "$$(uname -s)" = Darwin ] || { \
		printf '%s\n' 'error: $@ needs macOS; this host is not macOS.' >&2; \
		exit 1; \
	}

# The bundle needs its media libraries (bundle.sh refuses to build without a GStreamer
# prefix); both steps run with the GStreamer environment, and the prefix it names is passed on.
app-macos:
	$(REQUIRE_MACOS)
	$(GSTREAMER_ENV) "$(CARGO)" build --release --package "$(APP_PACKAGE)"
	$(GSTREAMER_ENV) scripts/macos/bundle.sh --out "$(DIST_DIR)" --gstreamer-prefix "$$MERCURY_GSTREAMER_PREFIX"

package-macos:
	$(REQUIRE_MACOS)
	$(GSTREAMER_ENV) "$(CARGO)" build --release --package "$(APP_PACKAGE)"
	$(GSTREAMER_ENV) scripts/macos/bundle.sh --out "$(DIST_DIR)" --gstreamer-prefix "$$MERCURY_GSTREAMER_PREFIX" --dmg

test-bundle-macos:
	$(REQUIRE_MACOS)
	scripts/macos/test-bundle.sh

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
