use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Debug, Default)]
pub(crate) struct FrameReadySignal {
    ready: Rc<Cell<bool>>,
}

impl FrameReadySignal {
    pub(crate) fn mark_ready(&self) {
        self.ready.set(true);
    }

    pub(crate) fn take_ready(&self) -> bool {
        self.ready.replace(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_signals_share_frame_readiness() {
        let signal = FrameReadySignal::default();
        let cloned_signal = signal.clone();

        cloned_signal.mark_ready();

        assert!(signal.take_ready());
        assert!(!cloned_signal.take_ready());
    }
}
