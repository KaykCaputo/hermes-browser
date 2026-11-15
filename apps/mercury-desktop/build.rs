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
