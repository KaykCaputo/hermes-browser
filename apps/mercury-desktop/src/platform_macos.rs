// Adapted from Servo, https://github.com/servo/servo, tag v0.8.0,
// ports/servoshell/platform/macos/mod.rs (the `INFO_PLIST` static), MPL-2.0.

//! The Info.plist embedded in the executable.
//!
//! macOS reads a `__TEXT,__info_plist` section as the property list of an executable that is not
//! inside an application bundle, so `target/release/mercury-browser` gets the application name in
//! the menu bar and HiDPI rendering just as `Mercury Browser.app` does. `build.rs` fills
//! `resources/macos/Info.plist.in`, the template the bundle's own Info.plist is made from.

/// The property list, kept in the binary by `#[used]` even though nothing reads it.
#[used]
#[unsafe(link_section = "__TEXT,__info_plist")]
static INFO_PLIST: [u8; GENERATED.len()] = match GENERATED.first_chunk() {
    Some(bytes) => *bytes,
    None => unreachable!(),
};

const GENERATED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/Info.plist"));

#[cfg(test)]
mod tests {
    use super::INFO_PLIST;

    fn embedded() -> &'static str {
        std::str::from_utf8(&INFO_PLIST).expect("the embedded Info.plist is UTF-8")
    }

    #[test]
    fn embedded_info_plist_has_every_placeholder_filled() {
        assert!(!embedded().contains("@VERSION@"));
        assert!(!embedded().contains("@MINIMUM_SYSTEM_VERSION@"));
    }

    #[test]
    fn embedded_info_plist_carries_the_workspace_version() {
        let version = format!("<string>{}</string>", env!("CARGO_PKG_VERSION"));
        assert!(embedded().contains(&format!(
            "<key>CFBundleShortVersionString</key>\n\t{version}"
        )));
        assert!(embedded().contains(&format!("<key>CFBundleVersion</key>\n\t{version}")));
    }

    #[test]
    fn embedded_info_plist_carries_the_deployment_target() {
        // Set by `.cargo/config.toml`.
        let target = env!("MACOSX_DEPLOYMENT_TARGET");
        assert!(embedded().contains(&format!(
            "<key>LSMinimumSystemVersion</key>\n\t<string>{target}</string>"
        )));
    }

    #[test]
    fn embedded_info_plist_names_the_executable_and_identifier() {
        assert!(embedded().contains("<string>mercury-browser</string>"));
        assert!(embedded().contains("<string>io.github.thepigdestroyer.mercury-browser</string>"));
    }
}
