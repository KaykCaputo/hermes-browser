use mercury_settings::SaveThrottle;

const INTERVAL: i64 = 15_000;
const T0: i64 = 1_763_200_000_000;

#[test]
fn nothing_changed_means_nothing_to_save() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    for now in [T0, T0 + INTERVAL, T0 + 10 * INTERVAL, 0, -5, i64::MAX] {
        assert!(!throttle.should_save(now), "{now}");
    }
    assert!(!throttle.is_dirty());
}

#[test]
fn a_change_is_saved_at_once_and_then_at_most_once_per_interval() {
    let mut throttle = SaveThrottle::new(INTERVAL);

    throttle.mark_dirty();
    assert!(throttle.is_dirty());
    assert!(throttle.should_save(T0));
    assert!(!throttle.should_save(T0), "true only once");
    throttle.saved(T0);
    assert!(!throttle.is_dirty());

    throttle.mark_dirty();
    assert!(!throttle.should_save(T0 + 1));
    assert!(!throttle.should_save(T0 + INTERVAL - 1));
    assert!(throttle.should_save(T0 + INTERVAL));
    throttle.saved(T0 + INTERVAL);

    assert!(
        !throttle.should_save(T0 + 5 * INTERVAL),
        "nothing new to save"
    );
}

#[test]
fn a_save_that_did_not_happen_is_offered_again_after_the_interval() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();

    assert!(throttle.should_save(T0));
    // The write failed, so `saved` is not called.
    assert!(!throttle.should_save(T0 + 1));
    assert!(throttle.is_dirty());
    assert!(throttle.should_save(T0 + INTERVAL));
}

#[test]
fn a_change_made_while_saving_is_not_forgotten() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    assert!(throttle.should_save(T0));
    throttle.mark_dirty(); // arrives while the earlier state is being written
    throttle.saved(T0 + 10);

    assert!(throttle.is_dirty());
    assert!(!throttle.should_save(T0 + 20));
    assert!(throttle.should_save(T0 + 10 + INTERVAL));
}

#[test]
fn a_flush_is_the_caller_saving_without_asking() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    assert!(throttle.should_save(T0));
    throttle.saved(T0);

    throttle.mark_dirty();
    assert!(!throttle.should_save(T0 + 1), "throttled");
    // On quit the caller writes anyway, then records it.
    throttle.saved(T0 + 2);

    assert!(!throttle.is_dirty());
    assert!(!throttle.should_save(T0 + 10 * INTERVAL));
}

#[test]
fn time_going_backwards_does_not_block_saves() {
    let mut throttle = SaveThrottle::new(INTERVAL);
    throttle.mark_dirty();
    assert!(throttle.should_save(T0));
    throttle.saved(T0);

    throttle.mark_dirty();
    // The clock was set back by a day.
    let earlier = T0 - 86_400_000;
    assert!(throttle.should_save(earlier));
    throttle.saved(earlier);

    throttle.mark_dirty();
    assert!(
        !throttle.should_save(earlier + 1),
        "throttling resumes from the new time"
    );
    assert!(throttle.should_save(earlier + INTERVAL));
}

#[test]
fn a_zero_or_negative_interval_saves_every_change() {
    for interval in [0, -1, i64::MIN] {
        let mut throttle = SaveThrottle::new(interval);
        for now in [T0, T0, T0 + 1] {
            throttle.mark_dirty();
            assert!(throttle.should_save(now), "{interval} {now}");
            throttle.saved(now);
        }
    }
}

#[test]
fn extreme_times_do_not_overflow() {
    let mut throttle = SaveThrottle::new(i64::MAX);
    throttle.mark_dirty();
    assert!(throttle.should_save(0));
    throttle.saved(0);

    throttle.mark_dirty();
    assert!(!throttle.should_save(1));
    assert!(throttle.should_save(i64::MAX));
    throttle.saved(i64::MAX);

    throttle.mark_dirty();
    assert!(
        throttle.should_save(i64::MIN),
        "a jump backwards is never a wait"
    );
}
