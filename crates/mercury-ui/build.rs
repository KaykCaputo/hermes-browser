fn main() -> Result<(), slint_build::CompileError> {
    // With the `test-support` feature (tests only), debug info names the elements so tests can
    // find them through Slint's testing backend. Never in a normal build: it also turns off
    // Slint's `optimize_useless_rectangles` pass and embeds element names.
    let debug_info = std::env::var_os("CARGO_FEATURE_TEST_SUPPORT").is_some();
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug_info);
    slint_build::compile_with_config("ui/main-window.slint", config)
}
