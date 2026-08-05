---
title: Troubleshooting
description: Diagnose startup, blank-page, graphics, media, and website compatibility problems in Hermes Browser.
---

# Troubleshooting

Start with the first unexpected message in the log. A later `Broken pipe` warning often describes a process that has already failed rather than the original cause.

## Collect useful logs

The default command keeps Hermes messages visible and reduces Servo noise:

```bash
make run
```

For a closer look:

```bash
make run RUST_LOG="talaria_servo=debug,servo=info"
```

Include the command, the first relevant error, your Linux distribution, display session, GPU, and the affected URL in a report. Remove personal URLs and tokens first.

## A page stays blank

Check whether the tab title or address changed.

- **Nothing changes:** inspect network, URL parsing, TLS, and startup errors.
- **The title changes but the page is blank:** loading and script execution probably succeeded, while the frame was not painted.
- **A first-frame message appears:** inspect the graphics context and compositor path.

Hermes deliberately uses Servo's single-process mode because the embedded cross-process Paint path in Servo 0.4.0 loads pages without displaying them. If your log says content-process isolation is enabled, you are not running the supported configuration from this repository.

## The browser opens on the wrong GPU

Hermes removes an inherited `DRI_PRIME` value before graphics or media threads start. Mesa and the desktop session choose the default adapter.

If rendering still uses the wrong device, inspect system-level Mesa configuration instead of adding a process-global override inside Hermes. Include `glxinfo -B` or the Wayland equivalent in the report when available.

## Media does not play

1. Confirm the page is using a direct media resource rather than Media Source Extensions.
2. Verify `playbin3`, the audio sink, and the required decoder with `gst-inspect-1.0`.
3. Run with detailed media logging.
4. Test the media URL in a small native GStreamer pipeline when possible.

YouTube is a known compatibility gap and is not a useful first test of the GStreamer backend.

## Build fails while compiling Servo

- Confirm that stable Rust is active with `rustup show active-toolchain`.
- Run `make doctor` to check the basic toolchain.
- Re-run `make install-linux-deps` on an apt-based Linux system.
- Check free disk space; Servo's native dependencies are large.
- Keep `Cargo.lock` unchanged and use the project's locked dependency graph.

## GLib cannot find a home directory

Warnings such as `getpwuid_r(): failed` or `HOME is not set` come from a process environment where GLib cannot resolve the current user. They can appear in an experimental sandbox. In the supported single-process configuration, make sure Hermes inherits a normal desktop environment and a valid `HOME` value.

## A particular website is broken

Reproduce the page in the Servo revision used by Hermes, ideally with Servo's `servoshell`. If it fails there too, the smallest useful fix probably belongs upstream. If `servoshell` works while Hermes fails, include that comparison in the Hermes issue.
