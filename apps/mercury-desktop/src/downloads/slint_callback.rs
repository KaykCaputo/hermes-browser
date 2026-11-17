//! A placeholder for the quit lane's `InSlintCallback`, replaced at merge: the quit lane's
//! type marks code running inside a closure that Slint calls, and its scan test requires
//! `InSlintCallback::enter()` as the first line of each. Here it does nothing.

/// Held for the duration of a Slint callback.
pub(crate) struct InSlintCallback;

impl InSlintCallback {
    #[must_use]
    pub(crate) fn enter() -> Self {
        Self
    }
}
