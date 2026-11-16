---
title: Continuous Integration
description: What Mercury Browser's CI and release workflows check, how to run the same checks locally, the dependency policy, and how to update the pins.
---

# Continuous integration

Every push to `master` or `mercury` and every pull request runs `.github/workflows/ci.yml` on Linux and on macOS (Apple Silicon). Other branches are checked through their pull request, so a push to a pull-request branch does not run the multi-hour macOS job twice. A tag `v*` runs `.github/workflows/release.yml`, which builds the release artefacts for both platforms and publishes them. Everything a job checks can also be checked on a development machine; see [Run the checks locally](#run-the-checks-locally).

## The jobs

The expected times below are estimates from the size of the work and the runners' specifications (GitHub's arm64 macOS runner has 3 M1 cores and 7 GB of memory; the Ubuntu runners have 4 cores and 16 GB on public repositories). Replace them with measured times once the workflows have run.

### `ci.yml` (pushes to `master` and `mercury`, pull requests, manual runs)

| Job | Runner | What it checks | Expected time |
| --- | --- | --- | --- |
| Documentation | `ubuntu-24.04` | `make docs` (MkDocs in strict mode) | 2 minutes |
| Fast checks (Linux) | `ubuntu-24.04` | `cargo fmt --check`; clippy with `-D warnings` and the tests of every crate that depends on neither Servo nor Slint | 3–6 minutes |
| Fast checks (macOS) | `macos-26` | the same, on Apple Silicon (some of those crates have macOS-only code) | 3–6 minutes |
| Linux quality checks | `ubuntu-22.04` | the whole workspace: `make fmt-check`, clippy with `-D warnings --all-features`, `cargo test` | 60–110 minutes (limit 120) |
| macOS (Apple Silicon) | `macos-26` | GStreamer set-up; `cargo fmt --check`; the release build; `make app-macos`; the bundle checks, media libraries included (`--require-media`); a rendering smoke test; clippy with `-D warnings`; the workspace tests | 3–4 hours (limit 330 minutes) |
| Scripts | `ubuntu-24.04` | `shellcheck -x` over every `*.sh` under `scripts/`; `python3 -m py_compile` over every `*.py` | 1 minute |
| Workflows | `ubuntu-24.04` | `actionlint` over `.github/workflows/`, which also runs shellcheck over every `run:` block | 1 minute |
| Dependency policy | `ubuntu-24.04` | `cargo-deny check` against `deny.toml` | 2 minutes |

The fast jobs exist so that most mistakes are reported in minutes rather than after an engine build. Their crate list is not written down anywhere: `scripts/ci/light-crates.sh` works it out from `cargo metadata` each time, so a new library crate is checked as soon as it is added, provided nothing it depends on (including its dev-dependencies) pulls in `servo` or `slint`. `scripts/ci/light-crates.sh --explain` prints the verdict for every member.

The macOS job builds, tests and lints with one Cargo profile, `release`, the profile the project uses everywhere, so the engine's code is generated once: `cargo test --release` reuses what `cargo build --release` produced. Clippy still type-checks the dependency graph separately, as it always does. Building and linting link against GStreamer but load none of its libraries. The test binaries do, and they have no rpath to them, so the test step sets `DYLD_FALLBACK_LIBRARY_PATH="$MERCURY_GSTREAMER_PREFIX/lib"` on the `cargo test` command line. It cannot come from an earlier step: macOS strips `DYLD_*` variables whenever a SIP-protected program such as `/bin/bash` starts. The `.app` is zipped and uploaded as an artefact before the smoke test, clippy and the tests, so a later failure or timeout cannot lose it. The smoke test runs the bundled app with `--screenshot` on `tests/pages/colors.html` and checks the picture with `scripts/ci/check-colors-screenshot.py`; the PNG is uploaded whether it passes or fails.

Superseded runs on the same branch or pull request are cancelled. Tags run only the release workflow, whose runs are never cancelled.

### `release.yml` (tags `v*`, manual runs)

| Job | Runner | What it does | Expected time |
| --- | --- | --- | --- |
| Check the version | `ubuntu-24.04` | reads `[workspace.package] version` from `Cargo.toml`; on a tag, fails unless the tag is `v<version>` | 1 minute |
| Linux x86-64 | `ubuntu-22.04` | `make qa`, the release build, the glibc 2.35 check, `mercury-browser-linux-x86_64.tar.gz` and its `.sha256` | 90–120 minutes (limit 120) |
| macOS Apple Silicon (build) | `macos-26` | GStreamer set-up and the release build, with no secrets and no environment; hands the unsigned executable to the next job and records its SHA-256 as a job output | 2–3 hours (limit 240 minutes) |
| macOS Apple Silicon (sign and package) | `macos-26` | in the `release` environment, only for `v*` tags, on a fresh runner with no build step: the executable checked against the recorded digest, GStreamer downloaded afresh, Servo's plugin lists from the pinned crate, `scripts/macos/bundle.sh` (Developer ID when the secrets exist, otherwise ad hoc), the bundle checks, notarisation and stapling when the notarisation secrets exist, the checks again on the mounted disk image; uploads `mercury-browser-<version>-macos-arm64.dmg` and its `.sha256` | 15–30 minutes, plus Apple's notarisation time (limit 90) |
| Publish GitHub Release | `ubuntu-24.04` | only for a push of a `v*` tag: verifies every `.sha256`, writes `SHA256SUMS`, attests build provenance for the files it lists, creates a draft release, uploads every file, then publishes it. An already published release is refused, not changed. | 3 minutes |

A manual run (`workflow_dispatch`) builds and uploads the artefacts to the run and publishes nothing. From a branch, the signing job is skipped (it runs only for `v*` tags, which the `release` environment also enforces), so a manual run from a branch produces the Linux archive and the unsigned macOS executable only. The build to use from a branch is the ad-hoc `.app` that CI's macOS job uploads on every run.

The macOS release is split in two because the build runs the build scripts and procedural macros of about 1,100 crates. They run on a runner that never holds a signing secret. The signing job takes in nothing that the build job or a third party could write, except the executable itself. That file is checked against the digest the build job recorded, and is only copied and signed, never run. In particular:

- **No cache.** A cache entry is a tar archive extracted with absolute paths, so a poisoned entry saved by the build job could overwrite the checkout or plant programs.
- **GStreamer is downloaded afresh** and checked against the checksums pinned in `scripts/macos/gstreamer.sh`. None of its programs is run, and nothing of it goes on `PATH`.
- **Servo's plugin lists** are taken from the `servo` crate, checked against `Cargo.lock`.

What prevents a rebuild there is that the job has no step that builds. The Rust toolchain is also taken off its `PATH`, and failing stand-ins for `cargo`, `rustc`, `rustup` and `make` go first on it. They only make a mistake fail loudly; they are not a control. Its secrets come only from the `release` environment, and each step receives only the secrets it uses. The publishing job checks every file it publishes against the digest recorded by the job that made it, and refuses any file it does not expect.

**The limit of all this:** a compromised build can still have *its own output* signed and notarised, because signing what the build produced is the point of the job. What the design does is keep the credentials themselves (the private key, the notarisation key or password) out of the build's reach, so they cannot be stolen or used for anything else.

### Supply-chain rules for the workflows

- Every action is pinned to a full commit SHA with its version in a comment, for example `actions/checkout@3d3c42e… # v7.0.1`. Dependabot (`.github/dependabot.yml`) proposes updates weekly, all actions in one pull request.
- The workflows' token is read-only (`contents: read`). Only the publishing job gets `contents: write`, `id-token: write` and `attestations: write`, which publishing and `actions/attest` need.
- No workflow uses `pull_request_target`. Event data that a contributor controls is never written into a `run:` script; values reach scripts through `env:`.
- `ci.yml` uses no secrets, so pull requests, including those from forks, never see one. Only the macOS signing job reads secrets, from the `release` environment, and only in the steps that need them; no step that runs cargo has any.
- `scripts/ci/github-env.sh` passes exactly the variables `scripts/macos/gstreamer.sh env` sets (`MERCURY_GSTREAMER_PREFIX`, `PKG_CONFIG`, `PKG_CONFIG_LIBDIR`, a `PATH` prefix, and the unsetting of `PKG_CONFIG_PATH`) to later steps, and refuses anything else.
- The linters and cargo-deny are official release binaries at pinned versions, downloaded by `scripts/ci/install-tools.sh`, which checks each archive's SHA-256 before unpacking it.

### What is cached

- **The GStreamer packages** (`~/Library/Caches/mercury-browser/gstreamer/**/*.pkg`), keyed on `scripts/macos/gstreamer.sh`. That script holds the GStreamer version and the packages' checksums, so changing either starts a new cache. Only the downloaded `.pkg` files are kept: they come from a server outside GitHub; unpacking them is quick. The cache is saved straight after `gstreamer.sh fetch` has verified the checksums, so a run that fails later does not make the next one download them again.
- **Not the Cargo build directory.** A release build of the engine is roughly 10–15 GB, more than GitHub's per-repository cache allowance, so saving it would evict everything else and would take a large part of the time it saves to upload and download. The macOS job therefore builds the engine from scratch on every run. If that becomes too slow, a compiler cache such as `sccache` with a size limit is the next thing to try.
- **Not the Cargo registry.** Downloading the crates takes a minute or two from crates.io.

## Run the checks locally

| Command | Does what CI's … does |
| --- | --- |
| `make qa` | Linux quality checks (`fmt-check`, `lint`, `test`) |
| `make qa-fast` | Fast checks: formatting, then clippy and tests for the crates `scripts/ci/light-crates.sh` lists |
| `make lint-scripts` | Scripts and Workflows jobs: shellcheck, `py_compile` and actionlint |
| `make deny` | Dependency policy job |
| `make gstreamer-macos app-macos` then `scripts/ci/check-macos-bundle.sh --signature adhoc --require-media "dist/Mercury Browser.app"` | the bundle checks of the macOS job (with `eval "$(scripts/macos/gstreamer.sh env)"` in the shell first) |
| `scripts/ci/check-colors-screenshot.py <png>` | the smoke-test check (see below for the screenshot) |

`make lint-scripts` and `make deny` need shellcheck, actionlint and cargo-deny. If they are not installed, the targets say so; `make ci-tools` downloads the pinned versions into `target/ci-tools/bin` (checksums verified, nothing installed system-wide), where both targets look first.

The smoke test, as CI runs it:

```bash
"dist/Mercury Browser.app/Contents/MacOS/mercury-browser" --screenshot /tmp/colors.png \
    --window-size 900x700 "file://$PWD/tests/pages/colors.html"
scripts/ci/check-colors-screenshot.py /tmp/colors.png
```

`--screenshot` does not take focus; the browser writes the PNG and exits. The checker finds the page below the browser's own toolbar by its red square, measures the square to learn the display scale, and then checks a red square at the top left, a blue one at the top right, a green bar along the bottom and white in between. It prints one line per check and exits 0 (matches), 1 (wrong picture) or 2 (not a usable PNG: truncated, corrupt, malformed, or larger than any screenshot could be). It needs only Python's standard library.

`scripts/ci/check-macos-bundle.sh` checks that every Mach-O file in the bundle is arm64 only and records exactly one minimum macOS version; that the executable's equals `LSMinimumSystemVersion` and no library's is newer; that `Info.plist` passes `plutil -lint`; that `codesign --verify --deep --strict` passes with the hardened runtime on; that the entitlements are exactly the file `scripts/macos/bundle.sh` uses for that kind of signature (ad hoc or Developer ID); and that `scripts/macos/test-bundle.sh` passes. `--signature adhoc` or `--signature developer-id` also requires that kind of signature. `--require-media`, which both workflows pass, makes a bundle without its media libraries fail. It checks that `Contents/MacOS/lib` holds `libgstreamer-1.0.0.dylib` and every plugin in Servo's plugin lists. The plugins come from `scripts/macos/gstreamer-plugins.sh`, the one definition the collector uses too: every plugin it lists must be present, and every plugin `gstreamer-plugins.sh --excluded` lists (by default `ogg`, see `MERCURY_GSTREAMER_EXCLUDE_PLUGINS`) must be absent. It also checks that the executable loads `@rpath/libgstreamer-1.0.0.dylib` and that its only rpath is `@executable_path/lib/`, and that no Mach-O file loads a library from outside the bundle other than the system's own.

## Dependency policy

`deny.toml` holds the policy and cargo-deny enforces it. It reads `Cargo.lock` and compiles nothing.

- **Advisories.** A known vulnerability anywhere in the graph fails the check. Yanked crates fail it. Unmaintained and unsound crates fail it when the workspace depends on them directly. Each ignored advisory names the crate path that brings it in and why it cannot be fixed here:
  - `RUSTSEC-2023-0071` (the Marvin timing attack on RSA): `rsa 0.10.0-rc.19` ← `servo-script-webcrypto` 0.8.0, which pins that exact pre-release for WebCrypto.
  - `RUSTSEC-2026-0459` (SQLite before 3.54.0 with a crafted `-wal` file): one bundled SQLite, `libsqlite3-sys 0.36`, serves Servo's own databases (`servo-net`, `servo-storage`) and `mercury-import`. The importer copies another browser's history database together with its `-wal` and opens the copy, which is the advisory's scenario with files another program wrote. The fix (`rusqlite` 0.41) cannot be taken while Servo requires `rusqlite 0.38`: `libsqlite3-sys` declares `links = "sqlite3"`, so only one version can be in the graph. The remedy is a Servo upgrade, or carrying a patched `libsqlite3-sys` that bundles SQLite 3.54.
  Review both whenever Servo is upgraded.
- **Bans.** Several versions of one crate are allowed (the engine's graph needs them). A `*` version requirement is reported. The intended level for that is "deny". It warns for now, because cargo-deny counts the workspace's own `path` dependencies as wildcards in any crate that is not marked `publish = false`, and no crate is marked yet. `publish = false` is added to every crate, and the level switched to "deny", when the parallel lanes are merged.
- **Sources.** Only crates.io. Git dependencies and other registries are refused. The vendored crates in `third_party/` are path dependencies and need no exception.
- **Licences.** An explicit allow-list of what the graph needs. No copyleft licence is on it. A crate that offers a copyleft licence as one alternative among permissive ones passes on its permissive terms. A licence that is not on the list fails the check until someone decides about it.

| Licence | Needed by, for example |
| --- | --- |
| Apache-2.0 | `ab_glyph`, `aws-lc-sys`, `clang-sys` (33 crates in all) |
| Apache-2.0 WITH LLVM-exception | `target-lexicon` |
| BSD-2-Clause | `rav1e`, `arrayref`, `av1-grain` |
| BSD-3-Clause | `brotli`, `encoding_rs`, `mozangle` |
| BSL-1.0 | `clipboard-win`, `error-code` |
| CC0-1.0 | `hexf-parse` |
| CDLA-Permissive-2.0 | `webpki-roots`, `webpki-root-certs` (Mozilla's CA certificate data) |
| ISC | `aws-lc-rs`, `rustls-webpki`, `libloading` |
| MIT | `aho-corasick` and about 200 others |
| MPL-2.0 | Servo's crates, `adblock`, `cssparser`, `mozjs`, and Mercury itself |
| NCSA | `libfuzzer-sys` (via `rav1e` ← `ravif` ← `image` ← Slint's compiler) |
| Unicode-3.0 | the `icu_*` crates |
| Zlib | `foldhash`, `slotmap` |

**Slint.** Slint's crates are offered under "GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0". Mercury Browser uses them under the *Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0*. `deny.toml` allows that licence for the twelve Slint crates by name (`i-slint-renderer-software` only in mercury-ui's dev graph, for the headless rendering examples), as per-crate exceptions, and for nothing else. GPL is never allowed. The licence (its text is in each Slint crate's `LICENSES/` directory) grants use in a desktop application on condition that the application either shows Slint's `AboutSlint` widget in an About dialog reachable from its top-level menu (or in a splash screen), or shows the "Made with Slint" badge on a public web page, preferably the download page. It does not allow distributing Slint on its own, use in embedded systems, or exposing Slint's APIs, and Slint's licence notices must not be removed from its source.

## Release secrets

The macOS signing job understands these secrets. Store them as secrets of the `release` **environment**, not as repository secrets, so that only that job can read them (see [The release environment](#the-release-environment)). All are optional; none is available to pull requests, to `ci.yml` or to any job that runs cargo.

| Secret | What it is |
| --- | --- |
| `MACOS_CERTIFICATE` | A "Developer ID Application" certificate with its private key, exported from Keychain Access as a `.p12` file and encoded with `base64 -i cert.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | The password chosen when exporting the `.p12` |
| `MACOS_SIGN_IDENTITY` | The identity's full name, as `security find-identity -v -p codesigning` prints it, for example `Developer ID Application: Example Ltd (ABCDE12345)` |
| `APPLE_API_KEY` | Preferred for notarisation: the contents of an App Store Connect API key file (`AuthKey_XXXXXXXXXX.p8`, the text including its BEGIN/END lines), from App Store Connect → Users and Access → Integrations |
| `APPLE_API_KEY_ID` | That key's ten-character key ID |
| `APPLE_API_ISSUER` | The issuer ID shown on the same page |
| `APPLE_ID` | Alternative for notarisation: the Apple ID used to notarise |
| `APPLE_TEAM_ID` | The ten-character team identifier |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple ID (appleid.apple.com → Sign-In and Security) |

- **Without the first three,** the app is signed ad hoc (`scripts/macos/bundle.sh`'s default), is not notarised, and the job summary says so. Setting only some of the three fails the job rather than quietly building ad hoc. The same holds for each notarisation trio, and notarisation secrets without signing secrets also fail.
- **With them,** the certificate is imported into a keychain created for the job under the runner's temporary directory. The key is imported as non-extractable (`security import -x`), and the `.p12` file is written under `umask 077` and removed straight after the import. The login keychain is never used. The app, its libraries and the disk image are signed with that identity, with the hardened runtime and a secure timestamp, and the bundle is checked before anything is sent to Apple. The keychain is deleted straight after the last signing or notarising step, and again at the end of the job as a backstop.
- **With an API key (preferred),** the disk image is submitted with `xcrun notarytool submit --key … --key-id … --issuer … --wait`. The key is written to a private file for that one step and removed afterwards, so no secret appears on a command line.
- **With an Apple ID instead,** the app-specific password is stored in the temporary keychain with `notarytool store-credentials`, and the submission reads it from there. If both routes are configured, the API key is used.
- **Either way,** the image is stapled and its checksum written again. The stapled image is then verified (`stapler validate`, `spctl`), mounted, and the app inside checked once more.

Secrets are passed only through `env:` to the steps that use them and are never printed. Two of them unavoidably appear on a command line while one command runs, where any process on the runner could list them: the `.p12` password (`security import -P`) and, on the Apple ID route only, the app-specific password (`notarytool store-credentials --password`). Neither command has another non-interactive way to take it. What limits the exposure is that the runner is fresh and no step of that job runs third-party code. The API-key route avoids the second.

### The release environment

The signing job declares `environment: release` and runs only for `v*` tags. GitHub creates the environment with no rules the first time the job runs. Before the first release, the repository owner sets it up under **Settings → Environments → release**:

1. **Deployment branches and tags:** choose "Selected branches and tags" and add a **tag** rule `v*`. Add no branch rule. Only a run for a `v*` tag can then use the environment; a run from any branch is refused, and its secrets are never handed to that branch's code.
2. **Required reviewers:** add yourself (or the people who may approve a release). The signing job then waits for an approval before it receives any secret, which gives a moment to look at what the build produced. **Do not enable "Prevent self-review" while there is one maintainer:** it stops the person who pushed the tag from approving, and with one maintainer nobody could approve the release.
3. **Environment secrets:** add the secrets above here, and delete any repository secrets of the same names.
4. Optionally untick "Allow administrators to bypass configured protection rules"; with one maintainer, who is also the administrator, this mostly guards against accidents.

Also add a **tag ruleset** (Settings → Rules → Rulesets → New tag ruleset) targeting `v*`. Restrict creation, update and deletion to the maintainer, through the bypass list, so that nobody else who can push to the repository can create a release tag. A release tag is what the environment trusts.

**Public versus private.** On GitHub Free, Pro and Team plans, required reviewers, administrator-bypass control and wait timers are available only for public repositories. Environment secrets also need a public repository on GitHub Free. Branch-and-tag deployment rules need a public repository, or Pro or Team for a private one. Artifact attestations (`actions/attest`) need a public repository unless the plan is Enterprise Cloud. If the repository is made private on Free, the release job loses the reviewer gate and the attestation step fails. On Pro or Team it keeps the tag rule and the environment secrets but loses the reviewer gate and, again, the attestation. Check GitHub's "Deployments and environments" reference before changing visibility.

## First push

Nothing in these workflows has run on GitHub yet. On the first push, watch the jobs in this order; they fail fastest and say most about the rest:

1. **Workflows, Scripts, Dependency policy** (about a minute each). actionlint and the pinned shellcheck may disagree with a newer runner image. cargo-deny clones the RustSec database on every run, and an advisory published since the last local run fails it.
2. **Fast checks, Linux and macOS** (minutes). These are the first proof that `rustup toolchain install` reads `rust-toolchain.toml` on the runners, and that the engine-free crates build on Linux.
3. **Linux quality checks** (up to 2 hours). The first compile of the Linux-only code since the fork, including the Vulkan frame import and anything other lanes changed under `cfg(target_os = "linux")`.
4. **macOS (Apple Silicon)** (3–4 hours). In order of likelihood, the things to check:
   - free disk (the `df -h` lines of "Free disk space"; GitHub documents 14 GB, and the engine's build directory is far larger);
   - memory while linking (7 GB);
   - the GStreamer download and `github-env.sh` accepting `gstreamer.sh env`'s variables;
   - whether the runner can render at all in the smoke test (it needs a window server and a GPU);
   - the tests finding GStreamer through `DYLD_FALLBACK_LIBRARY_PATH`;
   - the 330-minute limit.
5. **The release workflow.** Its first run is the first real release, because the tag must equal `v<version>` and a manual run from a branch skips the signing job. That is safe enough: nothing is published until both platforms have built, signed and passed their checks, and the release stays a draft until every file is uploaded. A failure leaves at most a draft, which can be deleted with the tag before trying again. Check, in order:
   - the environment's approval;
   - the signing job's `PATH` (the "Confirm that no build tool is reachable" step);
   - the executable matching the build's digest;
   - the fresh GStreamer download;
   - the certificate import (`-x`, `set-key-partition-list`);
   - notarisation through the API key (or `notarytool store-credentials --keychain` on the Apple ID route);
   - stapling;
   - the checks on the mounted image;
   - the provenance attestation (public repositories only);
   - the draft being published.

## Updating the pins

**The Rust toolchain.** Change `channel` in `rust-toolchain.toml`. Then run `rustup toolchain install` (it reads the file), `make qa` and `make qa-fast`, fix any new clippy findings, and commit the toolchain change together with those fixes. The workflows install whatever the file names.

**The actions.** Accept Dependabot's weekly pull request. To move one by hand, find the commit of the release tag:

```bash
git ls-remote --tags https://github.com/actions/checkout | grep 'refs/tags/v7.0.1'
```

If the tag is annotated, use the line ending in `^{}`. Replace both the SHA and the version comment.

**shellcheck, actionlint, cargo-deny.** Change the version at the top of `scripts/ci/install-tools.sh` and both checksums in the same script. Take them from the project's checksum file (actionlint's `checksums.txt`, cargo-deny's `.sha256` files) or, for shellcheck, which publishes none, from the SHA-256 digest GitHub shows for the asset on its release page. Then run `make ci-tools lint-scripts deny`.

**GStreamer.** Change the version and the package checksums in `scripts/macos/gstreamer.sh`, as that script describes. The CI cache key is that script's hash, so the next run downloads the new packages. Check that `scripts/ci/check-macos-bundle.sh` still passes: a library that needs a newer macOS than `LSMinimumSystemVersion` fails it.

**The runner image.** `macos-26`, `ubuntu-24.04` and `ubuntu-22.04` are named explicitly. To move, change the label in both workflows. GitHub's list of labels is in its "GitHub-hosted runners reference"; each image's installed software is in the `actions/runner-images` repository. Keep the Linux release on the oldest supported Ubuntu for the glibc baseline.
