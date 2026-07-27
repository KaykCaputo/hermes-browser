use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use servo::ServoBuilder;

type WakeCallback = dyn Fn() + Send + Sync + 'static;

#[derive(Clone)]
pub struct EventLoopWaker {
    callback: Arc<WakeCallback>,
}

impl EventLoopWaker {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }
}

impl servo::EventLoopWaker for EventLoopWaker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        (self.callback)();
    }
}

pub struct ServoRuntime {
    inner: servo::Servo,
}

impl ServoRuntime {
    pub fn new(event_loop_waker: EventLoopWaker) -> Self {
        ensure_crypto_provider();

        let inner = ServoBuilder::default()
            .event_loop_waker(Box::new(event_loop_waker))
            .build();

        Self { inner }
    }

    pub fn spin_event_loop(&self) {
        self.inner.spin_event_loop();
    }
}

fn ensure_crypto_provider() {
    if CryptoProvider::get_default().is_none() {
        let _installation_result = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}
