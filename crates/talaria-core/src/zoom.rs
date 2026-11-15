pub const DEFAULT_ZOOM_PERCENT: u16 = 100;
pub const ZOOM_STEPS: [u16; 16] = [
    30, 50, 67, 80, 90, 100, 110, 120, 133, 150, 170, 200, 240, 300, 400, 500,
];

const MIN_ZOOM_PERCENT: u16 = ZOOM_STEPS[0];
const MAX_ZOOM_PERCENT: u16 = ZOOM_STEPS[ZOOM_STEPS.len() - 1];

/// The next larger step. `current` is first clamped to 30–500; a value between steps moves
/// to the step above it, and 500 stays at 500. The result is always one of `ZOOM_STEPS`.
pub fn zoom_in(current: u16) -> u16 {
    let current = current.clamp(MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    ZOOM_STEPS
        .into_iter()
        .find(|&step| step > current)
        .unwrap_or(MAX_ZOOM_PERCENT)
}

/// The next smaller step. `current` is first clamped to 30–500; a value between steps moves
/// to the step below it, and 30 stays at 30. The result is always one of `ZOOM_STEPS`.
pub fn zoom_out(current: u16) -> u16 {
    let current = current.clamp(MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT);
    ZOOM_STEPS
        .into_iter()
        .rev()
        .find(|&step| step < current)
        .unwrap_or(MIN_ZOOM_PERCENT)
}
