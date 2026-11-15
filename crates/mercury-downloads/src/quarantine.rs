//! Marking finished downloads so macOS Gatekeeper checks them before they are first opened.

use std::path::Path;

use url::Url;

/// Marks `path` as a web download from `data_url`, linked from `origin_url`, so macOS asks
/// before the file is first opened. Failure is logged, not fatal. Does nothing on other systems.
pub(crate) fn mark_downloaded(path: &Path, data_url: &Url, origin_url: Option<&Url>) {
    #[cfg(target_os = "macos")]
    if let Err(error) = macos::mark(path, data_url, origin_url) {
        log::warn!("could not quarantine {}: {error}", path.display());
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (path, data_url, origin_url);
}

/// The URL as recorded in the quarantine database: without credentials or fragment.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn recorded(url: &Url) -> String {
    let mut url = url.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    url.into()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;

    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSDictionary, NSString, NSURL, NSURLQuarantinePropertiesKey, ns_string,
    };
    use url::Url;

    /// Sets `NSURLQuarantinePropertiesKey`, which has the system write the
    /// `com.apple.quarantine` attribute (and its database entry) for us.
    ///
    /// The keys are the documented values of Launch Services' `kLSQuarantine…Key` constants
    /// (`LSQuarantine.h`), which live in CoreServices rather than Foundation.
    pub(super) fn mark(
        path: &Path,
        data_url: &Url,
        origin_url: Option<&Url>,
    ) -> Result<(), String> {
        let path = path.to_str().ok_or("the path is not valid UTF-8")?;
        autoreleasepool(|_| {
            let file = NSURL::fileURLWithPath(&NSString::from_str(path));
            let agent = NSString::from_str("Mercury Browser");
            let data = NSURL::URLWithString(&NSString::from_str(&super::recorded(data_url)));
            let origin = origin_url.and_then(|origin| {
                NSURL::URLWithString(&NSString::from_str(&super::recorded(origin)))
            });

            let mut keys: Vec<&NSString> = vec![
                ns_string!("LSQuarantineAgentName"),
                ns_string!("LSQuarantineType"),
            ];
            let mut values: Vec<&AnyObject> =
                vec![&agent, ns_string!("LSQuarantineTypeWebDownload")];
            if let Some(data) = &data {
                keys.push(ns_string!("LSQuarantineDataURL"));
                values.push(data);
            }
            if let Some(origin) = &origin {
                keys.push(ns_string!("LSQuarantineOriginURL"));
                values.push(origin);
            }
            let properties = NSDictionary::from_slices(&keys, &values);

            // SAFETY: the quarantine-properties key takes an `NSDictionary` of property-list
            // values, which is what is passed; reading the extern key constant is sound because
            // Foundation defines it for the life of the process.
            unsafe {
                file.setResourceValue_forKey_error(Some(&properties), NSURLQuarantinePropertiesKey)
            }
            .map_err(|error| error.localizedDescription().to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::recorded;
    use url::Url;

    #[test]
    fn recorded_urls_carry_no_credentials_or_fragment() {
        let url = Url::parse("https://user:secret@example.com/a.zip?x=1#part").unwrap();
        assert_eq!(recorded(&url), "https://example.com/a.zip?x=1");
    }
}
