use talaria_core::{DEFAULT_ZOOM_PERCENT, ZOOM_STEPS, zoom_in, zoom_out};

#[test]
fn zoom_in_moves_to_the_next_step() {
    assert_eq!(zoom_in(100), 110);
    assert_eq!(zoom_in(133), 150);
}

#[test]
fn zoom_in_saturates() {
    assert_eq!(zoom_in(500), 500);
}

#[test]
fn zoom_out_moves_to_the_previous_step() {
    assert_eq!(zoom_out(100), 90);
}

#[test]
fn zoom_out_saturates() {
    assert_eq!(zoom_out(30), 30);
}

#[test]
fn off_step_values_snap_in_the_requested_direction() {
    assert_eq!(zoom_in(105), 110);
    assert_eq!(zoom_out(105), 100);
}

#[test]
fn the_default_zoom_is_one_of_the_steps() {
    assert_eq!(DEFAULT_ZOOM_PERCENT, 100);
    assert!(ZOOM_STEPS.contains(&DEFAULT_ZOOM_PERCENT));
}
