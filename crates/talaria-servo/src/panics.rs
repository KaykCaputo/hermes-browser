//! What a process-wide panic hook needs to know about Servo.

/// Servo catches some panics on purpose (for example while decoding a broken image); a panic
/// hook must not report those as crashes.
pub fn should_panic_hook_suppress_termination() -> bool {
    servo::should_panic_hook_suppress_termination()
}

/// Whether Servo runs single-process with `hard_fail`, where any panic should end the process.
/// Mercury runs with `hard_fail` off. Called before Servo is configured, it fixes Servo's options
/// at their defaults (`hard_fail` on, single process), exactly as servoshell's hook does, so a
/// panic during start-up still ends the process.
pub fn hard_fail_in_single_process() -> bool {
    let options = servo::opts::get();
    options.hard_fail && !options.multiprocess
}
