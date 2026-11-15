mod common;

use std::time::Duration;

use common::{Fixture, NOW};
use mercury_store::VisitTransition;

#[test]
fn a_write_waits_for_another_connection_instead_of_failing() {
    let Fixture {
        mut store,
        raw,
        dir: _dir,
    } = Fixture::new("busy");
    raw.execute_batch("BEGIN IMMEDIATE").unwrap();
    let holder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        raw.execute_batch("COMMIT").unwrap();
    });
    store
        .record_visit("https://a.example/", None, VisitTransition::Link, NOW)
        .expect("waits for the lock rather than failing with SQLITE_BUSY");
    holder.join().unwrap();
}
