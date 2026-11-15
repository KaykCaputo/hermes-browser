pub const DEFAULT_ZOOM_PERCENT: u16 = 100;
pub const ZOOM_STEPS: [u16; 16] = [
    30, 50, 67, 80, 90, 100, 110, 120, 133, 150, 170, 200, 240, 300, 400, 500,
];

/// The next larger step; values between steps move to the step above them.
pub fn zoom_in(current: u16) -> u16 {
    ZOOM_STEPS
        .into_iter()
        .find(|&step| step > current)
        .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
}

/// The next smaller step; values between steps move to the step below them.
pub fn zoom_out(current: u16) -> u16 {
    ZOOM_STEPS
        .into_iter()
        .rev()
        .find(|&step| step < current)
        .unwrap_or(ZOOM_STEPS[0])
}
