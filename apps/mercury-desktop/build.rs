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

    // On macOS the GStreamer libraries are shipped in a `lib` directory beside the
    // executable; this rpath lets the dynamic linker find them.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/lib/");
        record_gstreamer_version();
    }
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
    if let Ok(output) = output
        && output.status.success()
    {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !version.is_empty() {
            println!("cargo:rustc-env=MERCURY_GSTREAMER_VERSION={version}");
        }
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
