fn main() -> Result<(), slint_build::CompileError> {
    // Debug info names the elements, so that tests can find them through Slint's testing
    // backend (`ElementHandle`); it adds only those names to the code.
    let config = slint_build::CompilerConfiguration::new().with_debug_info(true);
    slint_build::compile_with_config("ui/main-window.slint", config)
}
