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

    // macOS: the Info.plist embedded in the executable (src/platform_macos.rs).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        generate_embedded_info_plist();
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
