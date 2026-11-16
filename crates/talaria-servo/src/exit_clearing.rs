//! Browsing data the user asked to have cleared when the browser quits.

use servo::StorageType;

use crate::session_data::run_contained;

/// What the engine clears at quit, before it saves its files. Nothing by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitClearing {
    /// Every cookie, and each site's local and session storage.
    pub cookies_and_site_data: bool,
    /// The HTTP cache. Servo 0.8.0 keeps it in memory only, so it never outlives the run; it
    /// is cleared anyway, so this stays true if Servo starts keeping it on disk.
    pub cache: bool,
}

impl ExitClearing {
    pub(crate) const fn clears_anything(self) -> bool {
        self.cookies_and_site_data || self.cache
    }
}

/// Clears what `clearing` names while Servo still runs, so that the files it writes at exit
/// no longer hold it. A failure is logged (see `run_contained`).
pub(crate) fn clear_at_exit(servo: &servo::Servo, clearing: ExitClearing) {
    if clearing.cookies_and_site_data {
        run_contained("clearing cookies and site data at exit", || {
            let site_data = servo.site_data_manager();
            site_data.clear_cookies(None);
            let storage = StorageType::Local | StorageType::Session;
            let sites = site_data
                .site_data(storage)
                .into_iter()
                .map(|site| site.name())
                .collect::<Vec<_>>();
            let sites = sites.iter().map(String::as_str).collect::<Vec<_>>();
            site_data.clear_site_data(&sites, storage);
            log::info!(
                "cleared cookies and the storage of {} site(s) at exit",
                sites.len()
            );
        });
    }
    if clearing.cache {
        run_contained("clearing the cache at exit", || {
            servo.network_manager().clear_cache();
            log::info!("cleared the HTTP cache at exit");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_cleared_by_default() {
        assert!(!ExitClearing::default().clears_anything());
        assert!(
            ExitClearing {
                cache: true,
                ..ExitClearing::default()
            }
            .clears_anything()
        );
        assert!(
            ExitClearing {
                cookies_and_site_data: true,
                ..ExitClearing::default()
            }
            .clears_anything()
        );
    }
}
