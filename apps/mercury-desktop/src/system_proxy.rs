//! Reads the macOS proxy settings (System Settings > Network > Details > Proxies) once at
//! start. Read-only: `SCDynamicStoreCopyProxies` needs no permission and changes nothing.

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_system_configuration::SCDynamicStore;
use talaria_servo::{ProxyServer, SystemProxySettings};

/// The keys of the proxies dictionary (`kSCPropNetProxies*` in `SCSchemaDefinitions.h`).
mod keys {
    pub const HTTP_ENABLE: &str = "HTTPEnable";
    pub const HTTP_PROXY: &str = "HTTPProxy";
    pub const HTTP_PORT: &str = "HTTPPort";
    pub const HTTPS_ENABLE: &str = "HTTPSEnable";
    pub const HTTPS_PROXY: &str = "HTTPSProxy";
    pub const HTTPS_PORT: &str = "HTTPSPort";
    pub const SOCKS_ENABLE: &str = "SOCKSEnable";
    pub const SOCKS_PROXY: &str = "SOCKSProxy";
    pub const SOCKS_PORT: &str = "SOCKSPort";
    pub const EXCEPTIONS_LIST: &str = "ExceptionsList";
    pub const EXCLUDE_SIMPLE_HOSTNAMES: &str = "ExcludeSimpleHostnames";
    pub const AUTO_CONFIG_ENABLE: &str = "ProxyAutoConfigEnable";
    pub const AUTO_CONFIG_URL: &str = "ProxyAutoConfigURLString";
    pub const AUTO_DISCOVERY_ENABLE: &str = "ProxyAutoDiscoveryEnable";
}

/// The system's proxy settings, or `None` if they cannot be read.
pub(crate) fn read_system_proxy() -> Option<SystemProxySettings> {
    let proxies = SCDynamicStore::proxies(None)?;
    Some(settings_from_dictionary(&proxies))
}

/// The proxies dictionary as plain data. Missing or mistyped entries count as "off".
fn settings_from_dictionary(proxies: &CFDictionary) -> SystemProxySettings {
    // SAFETY: every key and value of a CFDictionary is a CF object, so viewing them as
    // `CFType` is sound; values are only used after a checked `downcast`. The keys are
    // compared by the dictionary's own callbacks (`CFEqual`).
    let proxies: &CFDictionary<CFString, CFType> = unsafe { proxies.cast_unchecked() };
    let value = |key: &'static str| proxies.get(&CFString::from_static_str(key));
    let number = |key| {
        value(key)
            .and_then(|value| value.downcast::<CFNumber>().ok())
            .and_then(|number| number.as_i64())
    };
    let string = |key| {
        value(key)
            .and_then(|value| value.downcast::<CFString>().ok())
            .map(|string| string.to_string())
    };
    let enabled = |key| number(key) == Some(1);
    let server = |enable, host, port| {
        if !enabled(enable) {
            return None;
        }
        let port = number(port).and_then(|port| u16::try_from(port).ok());
        Some(ProxyServer::new(string(host).unwrap_or_default(), port))
    };
    let exceptions = value(keys::EXCEPTIONS_LIST)
        .and_then(|value| value.downcast::<CFArray>().ok())
        .map(|array| {
            // SAFETY: as above, every element of a CFArray is a CF object.
            let array: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(array) };
            array
                .iter()
                .filter_map(|entry| entry.downcast::<CFString>().ok())
                .map(|entry| entry.to_string())
                .collect()
        })
        .unwrap_or_default();

    SystemProxySettings {
        http: server(keys::HTTP_ENABLE, keys::HTTP_PROXY, keys::HTTP_PORT),
        https: server(keys::HTTPS_ENABLE, keys::HTTPS_PROXY, keys::HTTPS_PORT),
        socks: server(keys::SOCKS_ENABLE, keys::SOCKS_PROXY, keys::SOCKS_PORT),
        exceptions,
        exclude_simple_hostnames: enabled(keys::EXCLUDE_SIMPLE_HOSTNAMES),
        auto_config_url: enabled(keys::AUTO_CONFIG_ENABLE)
            .then(|| string(keys::AUTO_CONFIG_URL).unwrap_or_default()),
        auto_discovery: enabled(keys::AUTO_DISCOVERY_ENABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Value {
        Number(i32),
        Text(&'static str),
        List(&'static [&'static str]),
    }

    fn dictionary(entries: &[(&'static str, Value)]) -> CFRetained<CFDictionary> {
        let keys = entries
            .iter()
            .map(|(key, _)| CFString::from_static_str(key))
            .collect::<Vec<_>>();
        let values = entries
            .iter()
            .map(|(_, value)| -> CFRetained<CFType> {
                match value {
                    Value::Number(number) => CFNumber::new_i32(*number).into(),
                    Value::Text(text) => CFString::from_static_str(text).into(),
                    Value::List(list) => {
                        let strings = list
                            .iter()
                            .map(|entry| CFString::from_static_str(entry))
                            .collect::<Vec<_>>();
                        let array = CFArray::from_retained_objects(&strings);
                        // SAFETY: only forgets the element type.
                        let array: CFRetained<CFArray> =
                            unsafe { CFRetained::cast_unchecked(array) };
                        array.into()
                    }
                }
            })
            .collect::<Vec<_>>();
        let keys = keys.iter().map(|key| &**key).collect::<Vec<_>>();
        let values = values.iter().map(|value| &**value).collect::<Vec<_>>();
        let typed = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        // SAFETY: only forgets the element types.
        unsafe { CFRetained::cast_unchecked(typed) }
    }

    #[test]
    fn the_key_names_are_the_system_configuration_constants() {
        use objc2_system_configuration as sc;
        // SAFETY: the constants are immutable CFStrings exported by the framework.
        let constants = unsafe {
            [
                (sc::kSCPropNetProxiesHTTPEnable, keys::HTTP_ENABLE),
                (sc::kSCPropNetProxiesHTTPProxy, keys::HTTP_PROXY),
                (sc::kSCPropNetProxiesHTTPPort, keys::HTTP_PORT),
                (sc::kSCPropNetProxiesHTTPSEnable, keys::HTTPS_ENABLE),
                (sc::kSCPropNetProxiesHTTPSProxy, keys::HTTPS_PROXY),
                (sc::kSCPropNetProxiesHTTPSPort, keys::HTTPS_PORT),
                (sc::kSCPropNetProxiesSOCKSEnable, keys::SOCKS_ENABLE),
                (sc::kSCPropNetProxiesSOCKSProxy, keys::SOCKS_PROXY),
                (sc::kSCPropNetProxiesSOCKSPort, keys::SOCKS_PORT),
                (sc::kSCPropNetProxiesExceptionsList, keys::EXCEPTIONS_LIST),
                (
                    sc::kSCPropNetProxiesExcludeSimpleHostnames,
                    keys::EXCLUDE_SIMPLE_HOSTNAMES,
                ),
                (
                    sc::kSCPropNetProxiesProxyAutoConfigEnable,
                    keys::AUTO_CONFIG_ENABLE,
                ),
                (
                    sc::kSCPropNetProxiesProxyAutoConfigURLString,
                    keys::AUTO_CONFIG_URL,
                ),
                (
                    sc::kSCPropNetProxiesProxyAutoDiscoveryEnable,
                    keys::AUTO_DISCOVERY_ENABLE,
                ),
            ]
        };
        for (constant, name) in constants {
            assert_eq!(constant.to_string(), name);
        }
    }

    #[test]
    fn an_empty_dictionary_means_no_proxy() {
        assert_eq!(
            settings_from_dictionary(&dictionary(&[])),
            SystemProxySettings::default()
        );
    }

    #[test]
    fn enabled_proxies_and_the_bypass_list_are_read() {
        let proxies = dictionary(&[
            (keys::HTTP_ENABLE, Value::Number(1)),
            (keys::HTTP_PROXY, Value::Text("proxy.example")),
            (keys::HTTP_PORT, Value::Number(3128)),
            (keys::HTTPS_ENABLE, Value::Number(1)),
            (keys::HTTPS_PROXY, Value::Text("secure.example")),
            (keys::HTTPS_PORT, Value::Number(8443)),
            (
                keys::EXCEPTIONS_LIST,
                Value::List(&["*.local", "169.254/16"]),
            ),
            (keys::EXCLUDE_SIMPLE_HOSTNAMES, Value::Number(1)),
        ]);

        let settings = settings_from_dictionary(&proxies);

        assert_eq!(
            settings,
            SystemProxySettings {
                http: Some(ProxyServer::new("proxy.example", Some(3128))),
                https: Some(ProxyServer::new("secure.example", Some(8443))),
                exceptions: vec!["*.local".to_owned(), "169.254/16".to_owned()],
                exclude_simple_hostnames: true,
                ..SystemProxySettings::default()
            }
        );
        // And on to Servo's preferences.
        let preferences = settings.preferences();
        assert_eq!(preferences.http_uri, "http://proxy.example:3128");
        assert_eq!(preferences.https_uri, "http://secure.example:8443");
        assert_eq!(
            preferences.no_proxy,
            "localhost,127.0.0.0/8,::1,.local,169.254.0.0/16"
        );
    }

    #[test]
    fn a_configured_but_disabled_proxy_is_off() {
        // macOS keeps the host and port of a proxy that was switched off.
        let proxies = dictionary(&[
            (keys::HTTP_ENABLE, Value::Number(0)),
            (keys::HTTP_PROXY, Value::Text("proxy.example")),
            (keys::HTTP_PORT, Value::Number(3128)),
            (keys::HTTPS_PROXY, Value::Text("secure.example")),
        ]);

        let settings = settings_from_dictionary(&proxies);

        assert_eq!(settings, SystemProxySettings::default());
        assert_eq!(settings.preferences(), Default::default());
    }

    #[test]
    fn mistyped_values_count_as_missing() {
        let proxies = dictionary(&[
            (keys::HTTP_ENABLE, Value::Text("1")),
            (keys::HTTPS_ENABLE, Value::Number(1)),
            (keys::HTTPS_PROXY, Value::Number(7)),
            (keys::HTTPS_PORT, Value::Number(70_000)),
            (keys::EXCEPTIONS_LIST, Value::Text("*.local")),
        ]);

        assert_eq!(
            settings_from_dictionary(&proxies),
            SystemProxySettings {
                https: Some(ProxyServer::new("", None)),
                ..SystemProxySettings::default()
            }
        );
    }

    #[test]
    fn socks_and_automatic_configuration_are_read_for_the_log() {
        let proxies = dictionary(&[
            (keys::SOCKS_ENABLE, Value::Number(1)),
            (keys::SOCKS_PROXY, Value::Text("socks.example")),
            (keys::SOCKS_PORT, Value::Number(1080)),
            (keys::AUTO_CONFIG_ENABLE, Value::Number(1)),
            (
                keys::AUTO_CONFIG_URL,
                Value::Text("http://wpad.example/proxy.pac"),
            ),
            (keys::AUTO_DISCOVERY_ENABLE, Value::Number(1)),
        ]);

        let settings = settings_from_dictionary(&proxies);

        assert_eq!(
            settings,
            SystemProxySettings {
                socks: Some(ProxyServer::new("socks.example", Some(1080))),
                auto_config_url: Some("http://wpad.example/proxy.pac".to_owned()),
                auto_discovery: true,
                ..SystemProxySettings::default()
            }
        );
        assert_eq!(settings.preferences(), Default::default());
        assert_eq!(settings.unsupported().len(), 3);
    }

    #[test]
    fn this_machines_settings_can_be_read() {
        // Whatever they are; only reads them.
        assert!(read_system_proxy().is_some());
    }
}
