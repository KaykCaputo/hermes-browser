use mercury_settings::{SaveThrottle, SaveToken};

const INTERVAL: i64 = 15_000;
const T0: i64 = 1_763_200_000_000;

fn token(throttle: &mut SaveThrottle, now: i64) -> SaveToken {
    throttle
        .should_save(now)
        .unwrap_or_else(|| panic!("a save should be offered at {now}"))
}

fn offered(throttle: &mut SaveThrottle, now: i64) -> bool {
    throttle.should_save(now).is_some()
}

#[test]
fn nothing_changed_means_nothing_to_save() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    for now in [T0, T0 + INTERVAL, T0 + 10 * INTERVAL, 0, -5, i64::MAX] {
        assert!(!offered(&mut throttle, now), "{now}");
    }
    assert!(!throttle.is_dirty());
}

#[test]
fn a_change_is_saved_at_once_and_then_at_most_once_per_interval() {
    let mut throttle = SaveThrottle::new(INTERVAL);

    throttle.mark_dirty();
    assert!(throttle.is_dirty());
    let first = token(&mut throttle, T0);
    assert!(!offered(&mut throttle, T0), "offered only once");
    throttle.saved(first, T0);
    assert!(!throttle.is_dirty());

    throttle.mark_dirty();
    assert!(!offered(&mut throttle, T0 + 1));
    assert!(!offered(&mut throttle, T0 + INTERVAL - 1));
    let second = token(&mut throttle, T0 + INTERVAL);
    throttle.saved(second, T0 + INTERVAL);

    assert!(
        !offered(&mut throttle, T0 + 5 * INTERVAL),
        "nothing new to save"
    );
}

#[test]
fn a_save_that_did_not_happen_is_offered_again_after_the_interval() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();

    let failed = token(&mut throttle, T0);
    drop(failed); // The write failed: the token is not handed back.
    assert!(!offered(&mut throttle, T0 + 1));
    assert!(throttle.is_dirty());
    assert!(offered(&mut throttle, T0 + INTERVAL));
}

#[test]
fn a_change_made_while_saving_is_not_forgotten() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    let saving = token(&mut throttle, T0);
    throttle.mark_dirty(); // arrives while the earlier state is being written
    throttle.saved(saving, T0 + 10);

    assert!(throttle.is_dirty());
    assert!(!offered(&mut throttle, T0 + 20));
    assert!(offered(&mut throttle, T0 + 10 + INTERVAL));
}

#[test]
fn a_late_token_does_not_undo_a_later_flush() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    let slow = token(&mut throttle, T0);
    throttle.mark_dirty();
    throttle.flushed(T0 + 1);
    throttle.saved(slow, T0 + 2); // the slower, older write reports last

    assert!(!throttle.is_dirty());
}

#[test]
fn a_flush_records_everything_without_asking() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    let first = token(&mut throttle, T0);
    throttle.saved(first, T0);

    throttle.mark_dirty();
    throttle.mark_dirty();
    assert!(!offered(&mut throttle, T0 + 1), "throttled");
    // On quit the caller writes anyway, then records it.
    throttle.flushed(T0 + 2);

    assert!(!throttle.is_dirty());
    assert!(!offered(&mut throttle, T0 + 10 * INTERVAL));
}

#[test]
fn a_flush_with_an_outstanding_token_counts_everything() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    let _abandoned = token(&mut throttle, T0);
    throttle.mark_dirty();

    throttle.flushed(T0 + 1);

    assert!(!throttle.is_dirty());
}

#[test]
fn time_going_backwards_does_not_block_saves() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    let first = token(&mut throttle, T0);
    throttle.saved(first, T0);

    throttle.mark_dirty();
    // The clock was set back by a day.
    let earlier = T0 - 86_400_000;
    let second = token(&mut throttle, earlier);
    throttle.saved(second, earlier);

    throttle.mark_dirty();
    assert!(
        !offered(&mut throttle, earlier + 1),
        "throttling resumes from the new time"
    );
    assert!(offered(&mut throttle, earlier + INTERVAL));
}

#[test]
fn a_zero_or_negative_interval_saves_every_change() {
    for interval in [0, -1, i64::MIN] {
        let mut throttle = SaveThrottle::new(interval);
        for now in [T0, T0, T0 + 1] {
            throttle.mark_dirty();
            let saving = token(&mut throttle, now);
            throttle.saved(saving, now);
        }
    }
}

#[test]
fn extreme_times_do_not_overflow() {
    let mut throttle = SaveThrottle::new(i64::MAX);
    throttle.mark_dirty();
    let first = token(&mut throttle, 0);
    throttle.saved(first, 0);

    throttle.mark_dirty();
    assert!(!offered(&mut throttle, 1));
    let second = token(&mut throttle, i64::MAX);
    throttle.saved(second, i64::MAX);

    throttle.mark_dirty();
    assert!(
        offered(&mut throttle, i64::MIN),
        "a jump backwards is never a wait"
    );
}
