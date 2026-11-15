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

#[test]
fn values_below_the_range_are_clamped_before_stepping() {
    assert_eq!(zoom_in(10), 50);
    assert_eq!(zoom_out(10), 30);
    assert_eq!(zoom_in(0), 50);
}

#[test]
fn values_above_the_range_are_clamped_before_stepping() {
    assert_eq!(zoom_in(600), 500);
    assert_eq!(zoom_out(600), 400);
    assert_eq!(zoom_out(u16::MAX), 400);
}

#[test]
fn values_between_steps_move_to_the_neighbouring_step() {
    assert_eq!(zoom_in(95), 100);
    assert_eq!(zoom_out(95), 90);
    assert_eq!(zoom_in(450), 500);
    assert_eq!(zoom_out(450), 400);
}

#[test]
fn every_result_is_a_step() {
    for value in [0, 29, 30, 31, 99, 100, 101, 499, 500, 501, u16::MAX] {
        assert!(ZOOM_STEPS.contains(&zoom_in(value)), "zoom_in({value})");
        assert!(ZOOM_STEPS.contains(&zoom_out(value)), "zoom_out({value})");
    }
}
