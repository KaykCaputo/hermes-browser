#[cfg(target_os = "macos")]
mod metal;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod software;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod surfman;
#[cfg(target_os = "linux")]
mod vulkan;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::surfman::BrowserRenderingContext;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use software::BrowserRenderingContext;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::surfman::RenderingBridgeError;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use software::RenderingBridgeError;

/// Lets an error that recurs every frame be reported once for each kind (enum variant).
pub(crate) struct ReportOncePerKind<E> {
    reported: Vec<std::mem::Discriminant<E>>,
}

impl<E> Default for ReportOncePerKind<E> {
    fn default() -> Self {
        Self {
            reported: Vec::new(),
        }
    }
}

impl<E> ReportOncePerKind<E> {
    /// `true` the first time an error of this kind is seen.
    pub(crate) fn first(&mut self, error: &E) -> bool {
        let kind = std::mem::discriminant(error);
        if self.reported.contains(&kind) {
            return false;
        }
        self.reported.push(kind);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::ReportOncePerKind;

    #[allow(dead_code)]
    enum Failure {
        Import(String),
        Missing,
    }

    #[test]
    fn each_kind_of_error_is_reported_once() {
        let mut reports = ReportOncePerKind::default();

        assert!(reports.first(&Failure::Import("first".to_owned())));
        assert!(!reports.first(&Failure::Import("second message".to_owned())));
        assert!(reports.first(&Failure::Missing));
        assert!(!reports.first(&Failure::Missing));
        assert!(!reports.first(&Failure::Import("third".to_owned())));
    }
}
