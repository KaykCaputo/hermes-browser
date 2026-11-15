use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// How long Servo may take to shut down before the process gives up waiting for it.
pub(crate) const ENGINE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the engine owns that must be released in a fixed order at shutdown.
pub(crate) struct EngineParts<W, R, C, D> {
    /// Web views and anything that only serves them, such as their event receivers.
    pub(crate) web_views: W,
    pub(crate) runtime: Option<R>,
    pub(crate) rendering_context: C,
    pub(crate) wgpu_device: D,
}

/// Releases the engine's parts in the one order that is safe, whatever order the fields are
/// declared in: closing the web views and then dropping the runtime runs Servo's own shutdown
/// (constellation exit, WebGL exit, WebRender teardown), which still needs the rendering
/// context and the wgpu device it renders through; those go last.
pub(crate) fn release_in_order<W, R, C, D>(
    parts: EngineParts<W, R, C, D>,
    drop_runtime: impl FnOnce(R),
) {
    let EngineParts {
        web_views,
        runtime,
        rendering_context,
        wgpu_device,
    } = parts;

    drop(web_views);
    if let Some(runtime) = runtime {
        drop_runtime(runtime);
    }
    drop(rendering_context);
    drop(wgpu_device);
}

/// How long Servo keeps running between closing the web views and dropping the runtime.
///
/// This mitigates a race in Servo 0.8's WebGL thread; it is not a fix. A page's WebGL contexts
/// are destroyed only when its content process, closing the page, sends `RemoveContext`
/// (servo-script-0.8.0/dom/webgl/webglrenderingcontext.rs:168-171). Dropping the last `Servo`
/// handle right after closing the web views stops the constellation while that is still in
/// flight. The WebGL thread's `recv()` then returns an error, its loop ends
/// (servo-webgl-0.8.0/webgl_thread.rs:319-327) before `WebGLMsg::Exit` is ever sent, and the
/// thread drops contexts that are still alive, which surfman turns into a panic
/// (third_party/surfman/src/cgl/context.rs:46). The real fix belongs in that WebGL thread.
///
/// There is no signal to wait on instead: `WebViewDelegate::notify_closed` reports only
/// `window.close()` from page content, the constellation's `WebViewClosed` and
/// `PipelineExited` messages are handled inside Servo (servo-0.8.0/servo.rs:882-886, 936-946)
/// and never reach the embedder for a web view the embedder dropped, and nothing reports
/// that the WebGL thread has released a context.
pub(crate) const WEBGL_SETTLE_PERIOD: Duration = Duration::from_millis(500);
const SETTLE_SPIN_INTERVAL: Duration = Duration::from_millis(5);

/// No web view means no content process and no WebGL context, so nothing to wait for.
pub(crate) fn settle_period(web_view_created: bool) -> Duration {
    if web_view_created {
        WEBGL_SETTLE_PERIOD
    } else {
        Duration::ZERO
    }
}

/// Calls `spin` repeatedly until `period` has passed.
pub(crate) fn spin_for(period: Duration, mut spin: impl FnMut()) {
    let started = Instant::now();
    while started.elapsed() < period {
        spin();
        thread::sleep(SETTLE_SPIN_INTERVAL);
    }
}

/// Drops `value`, catching a panic raised while dropping it. Returns whether the drop was clean.
///
/// This is a last guard so that a panic in the engine's teardown does not replace the exit
/// code; it fixes nothing. Its limit: a second panic raised while the first one unwinds
/// aborts the process, and nothing can catch that.
pub(crate) fn drop_catching_panic<T>(value: T) -> bool {
    std::panic::catch_unwind(AssertUnwindSafe(move || drop(value))).is_ok()
}

/// Runs `on_timeout` on a separate thread unless `disarm` is called within `timeout`.
pub(crate) struct Watchdog {
    disarm: Sender<()>,
}

impl Watchdog {
    pub(crate) fn start(timeout: Duration, on_timeout: impl FnOnce() + Send + 'static) -> Self {
        let (disarm, disarmed) = mpsc::channel();
        let spawn_result = thread::Builder::new()
            .name("shutdown watchdog".to_owned())
            .spawn(move || {
                // A dropped watchdog counts as disarmed; only silence for `timeout` fires it.
                if disarmed.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                    on_timeout();
                }
            });
        if let Err(error) = spawn_result {
            log::warn!("could not start the shutdown watchdog: {error}");
        }
        Self { disarm }
    }

    pub(crate) fn disarm(self) {
        let _sent = self.disarm.send(());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::*;

    struct Recorded {
        name: &'static str,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Drop for Recorded {
        fn drop(&mut self) {
            self.log.borrow_mut().push(self.name);
        }
    }

    fn recorded(name: &'static str, log: &Rc<RefCell<Vec<&'static str>>>) -> Recorded {
        Recorded {
            name,
            log: Rc::clone(log),
        }
    }

    #[test]
    fn web_views_then_runtime_then_rendering_context_then_device() {
        let log = Rc::new(RefCell::new(Vec::new()));
        // Declared in the reverse of the required order, so field order cannot be what
        // makes the test pass.
        let parts = EngineParts {
            wgpu_device: recorded("wgpu device", &log),
            rendering_context: recorded("rendering context", &log),
            runtime: Some(recorded("runtime", &log)),
            web_views: recorded("web views", &log),
        };

        release_in_order(parts, drop);

        assert_eq!(
            *log.borrow(),
            vec!["web views", "runtime", "rendering context", "wgpu device"]
        );
    }

    #[test]
    fn the_runtime_is_released_through_the_given_function() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let parts = EngineParts {
            web_views: (),
            runtime: Some(recorded("runtime", &log)),
            rendering_context: recorded("rendering context", &log),
            wgpu_device: (),
        };
        let seen = Rc::clone(&log);

        release_in_order(parts, move |runtime| {
            seen.borrow_mut().push("before runtime");
            drop(runtime);
            seen.borrow_mut().push("after runtime");
        });

        assert_eq!(
            *log.borrow(),
            vec![
                "before runtime",
                "runtime",
                "after runtime",
                "rendering context"
            ]
        );
    }

    #[test]
    fn a_missing_runtime_is_skipped() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let parts = EngineParts {
            web_views: recorded("web views", &log),
            runtime: None::<Recorded>,
            rendering_context: recorded("rendering context", &log),
            wgpu_device: recorded("wgpu device", &log),
        };

        release_in_order(parts, |_| panic!("there is no runtime to release"));

        assert_eq!(
            *log.borrow(),
            vec!["web views", "rendering context", "wgpu device"]
        );
    }

    #[test]
    fn the_settle_period_is_skipped_when_no_web_view_was_created() {
        assert_eq!(settle_period(false), Duration::ZERO);
        assert_eq!(settle_period(true), WEBGL_SETTLE_PERIOD);
    }

    #[test]
    fn spinning_for_a_period_keeps_spinning_until_it_has_passed() {
        let started = Instant::now();
        let mut spins = 0;

        spin_for(Duration::from_millis(60), || spins += 1);

        assert!(started.elapsed() >= Duration::from_millis(60));
        assert!(spins >= 2, "spun only {spins} times");
    }

    #[test]
    fn spinning_for_no_time_does_not_spin() {
        let mut spins = 0;

        spin_for(Duration::ZERO, || spins += 1);

        assert_eq!(spins, 0);
    }

    struct PanicsOnDrop;

    impl Drop for PanicsOnDrop {
        fn drop(&mut self) {
            panic!("deliberate panic while dropping");
        }
    }

    #[test]
    fn a_panic_while_dropping_is_caught_and_reported() {
        assert!(!drop_catching_panic(PanicsOnDrop));
    }

    #[test]
    fn a_clean_drop_is_reported_as_clean() {
        let log = Rc::new(RefCell::new(Vec::new()));

        assert!(drop_catching_panic(recorded("value", &log)));
        assert_eq!(*log.borrow(), vec!["value"]);
    }

    #[test]
    fn a_disarmed_watchdog_never_fires() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fired);
        let watchdog = Watchdog::start(Duration::from_millis(50), move || {
            flag.store(true, Ordering::SeqCst);
        });

        watchdog.disarm();
        thread::sleep(Duration::from_millis(150));

        assert!(!fired.load(Ordering::SeqCst));
    }

    #[test]
    fn a_watchdog_that_is_not_disarmed_in_time_fires() {
        let (sender, receiver) = mpsc::channel();
        let _watchdog = Watchdog::start(Duration::from_millis(20), move || {
            let _sent = sender.send(());
        });

        assert!(receiver.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
