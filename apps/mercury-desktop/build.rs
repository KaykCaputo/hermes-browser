// The macOS rpath below is adapted from Servo (https://github.com/servo/servo, tag v0.8.0,
// ports/servoshell/build.rs), MPL-2.0.

use std::env;
use std::fs::File;
use std::path::Path;

use gl_generator::{Api, Fallbacks, Profile, Registry, StructGenerator};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Only the Linux Vulkan frame import uses these bindings. Check the target, not the host.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        generate_gl_bindings();
    }

    // macOS: the Info.plist embedded in the executable (src/platform_macos.rs); the rpath
    // through which the dynamic linker finds the GStreamer libraries shipped in a `lib`
    // directory beside the executable; and the GStreamer version that names the private
    // registry (src/media_isolation.rs).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        generate_embedded_info_plist();
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/lib/");
        record_gstreamer_version();
    }
}

/// Fills `resources/macos/Info.plist.in` the way `scripts/macos/bundle.sh` does, so the bare
/// executable and `Mercury Browser.app` describe themselves identically.
fn generate_embedded_info_plist() {
    const TEMPLATE: &str = "resources/macos/Info.plist.in";
    println!("cargo:rerun-if-changed={TEMPLATE}");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let template =
        std::fs::read_to_string(TEMPLATE).expect("failed to read the Info.plist template");
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must provide CARGO_PKG_VERSION");
    // `.cargo/config.toml` sets this; without it rustc's arm64 default, 11.0, is what the linker uses.
    let minimum_system_version =
        env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".to_owned());
    let info_plist = template
        .replace("@VERSION@", &version)
        .replace("@MINIMUM_SYSTEM_VERSION@", &minimum_system_version);

    let output_directory = env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR");
    std::fs::write(Path::new(&output_directory).join("Info.plist"), info_plist)
        .expect("failed to write the embedded Info.plist");
}

/// The GStreamer version the media backend is built against, which names the private registry
/// (src/media_isolation.rs). It comes from the pkg-config the gstreamer-sys crates use.
fn record_gstreamer_version() {
    for variable in ["PKG_CONFIG", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_PATH"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let pkg_config = env::var_os("PKG_CONFIG").unwrap_or_else(|| "pkg-config".into());
    let output = std::process::Command::new(pkg_config)
        .args(["--modversion", "gstreamer-1.0"])
        .output();
    let version = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => String::new(),
    };
    if version.is_empty() {
        // The media registry would then be named "unknown"; a build that links GStreamer
        // normally finds it through the same pkg-config, so this points at a broken setup.
        println!(
            "cargo:warning=could not read the GStreamer version with pkg-config; \
             run the build with the environment of scripts/macos/gstreamer.sh env"
        );
    } else {
        println!("cargo:rustc-env=MERCURY_GSTREAMER_VERSION={version}");
    }
}

fn generate_gl_bindings() {
    let output_directory = env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR");
    let output_path = Path::new(&output_directory).join("gl_bindings.rs");
    let mut output = File::create(output_path).expect("failed to create OpenGL bindings");

    Registry::new(
        Api::Gles2,
        (3, 0),
        Profile::Core,
        Fallbacks::All,
        ["GL_EXT_memory_object", "GL_EXT_memory_object_fd"],
    )
    .write_bindings(StructGenerator, &mut output)
    .expect("failed to generate OpenGL bindings");
}
