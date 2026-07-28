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

    pub(crate) fn has_frame(&self) -> bool {
        self.ready.get()
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

        assert!(signal.has_frame());
        assert!(cloned_signal.has_frame());
    }
}
