use std::env;
use std::fs::File;
use std::path::Path;

use gl_generator::{Api, Fallbacks, Profile, Registry, StructGenerator};

fn main() {
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

    println!("cargo:rerun-if-changed=build.rs");
}
