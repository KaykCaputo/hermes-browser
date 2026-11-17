//! The system's proxy settings, mapped onto Servo's proxy preferences.

/// A proxy server named by the system settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyServer {
    pub host: String,
    pub port: Option<u16>,
}

impl ProxyServer {
    pub fn new(host: impl Into<String>, port: Option<u16>) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// The system's proxy settings as plain data: on macOS, what `SCDynamicStoreCopyProxies`
/// reports. A server is `Some` only if the system has it turned on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemProxySettings {
    /// "Web proxy (HTTP)".
    pub http: Option<ProxyServer>,
    /// "Secure web proxy (HTTPS)": an HTTP proxy used for `https` addresses.
    pub https: Option<ProxyServer>,
    /// "SOCKS proxy". Servo 0.8.0 cannot use one.
    pub socks: Option<ProxyServer>,
    /// "Bypass proxy settings for these hosts & domains".
    pub exceptions: Vec<String>,
    /// "Exclude simple hostnames".
    pub exclude_simple_hostnames: bool,
    /// "Automatic proxy configuration": the PAC file's address. Servo 0.8.0 cannot use one.
    pub auto_config_url: Option<String>,
    /// "Auto proxy discovery" (WPAD). Servo 0.8.0 cannot use it.
    pub auto_discovery: bool,
}

/// Values for Servo's `network_http_proxy_uri`, `network_https_proxy_uri` and
/// `network_http_no_proxy`; an empty string means "none".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyPreferences {
    pub http_uri: String,
    pub https_uri: String,
    pub no_proxy: String,
}

/// Always bypass the system proxy, as other browsers do: a proxy cannot reach this machine's
/// own servers.
const LOOPBACK_BYPASS: [&str; 3] = ["localhost", "127.0.0.0/8", "::1"];

impl SystemProxySettings {
    /// What Servo should use. Servo's proxy connector (`servo-net` 0.8.0 `ProxyConnector`)
    /// opens a `CONNECT` tunnel through the proxy for both schemes, so both are given as
    /// `http://host:port`. The bypass list is written in the syntax of `hyper-util`'s
    /// `Matcher::no`: a domain matches itself and its subdomains, IP addresses and CIDR
    /// networks match, `*` alone matches everything; nothing else is a wildcard.
    pub fn preferences(&self) -> ProxyPreferences {
        let http_uri = proxy_uri(self.http.as_ref()).unwrap_or_default();
        let https_uri = proxy_uri(self.https.as_ref()).unwrap_or_default();
        if http_uri.is_empty() && https_uri.is_empty() {
            return ProxyPreferences::default();
        }
        let no_proxy = LOOPBACK_BYPASS
            .iter()
            .map(|entry| (*entry).to_owned())
            .chain(
                self.exceptions
                    .iter()
                    .filter_map(|entry| bypass_entry(entry).ok().flatten()),
            )
            .collect::<Vec<_>>()
            .join(",");
        ProxyPreferences {
            http_uri,
            https_uri,
            no_proxy,
        }
    }

    /// One message for each setting in use that Mercury cannot honour, for the log.
    pub fn unsupported(&self) -> Vec<String> {
        let mut messages = Vec::new();
        for (name, server) in [
            ("web proxy (HTTP)", &self.http),
            ("secure web proxy (HTTPS)", &self.https),
        ] {
            if let Some(server) = server
                && proxy_uri(Some(server)).is_none()
            {
                messages.push(format!(
                    "the system's {name} host {:?} is not a host name; it is not used",
                    server.host
                ));
            }
        }
        let uses_a_proxy =
            !self.preferences().http_uri.is_empty() || !self.preferences().https_uri.is_empty();
        if uses_a_proxy {
            for entry in &self.exceptions {
                if bypass_entry(entry).is_err() {
                    messages.push(format!(
                        "the proxy bypass entry {entry:?} has a wildcard Mercury cannot match; \
                         it is ignored"
                    ));
                }
            }
            if self.exclude_simple_hostnames {
                messages.push(
                    "\"exclude simple host names\" is not supported; only localhost bypasses \
                     the proxy"
                        .to_owned(),
                );
            }
        }
        if let Some(server) = &self.socks {
            messages.push(format!(
                "the system's SOCKS proxy ({}) is not supported by the engine; connections it \
                 would carry are made {}",
                server.host,
                if uses_a_proxy {
                    "through the web proxies or directly"
                } else {
                    "directly"
                }
            ));
        }
        if let Some(url) = &self.auto_config_url {
            messages.push(format!(
                "automatic proxy configuration ({url}) is not supported by the engine; it is \
                 ignored"
            ));
        }
        if self.auto_discovery {
            messages.push(
                "automatic proxy discovery (WPAD) is not supported by the engine; it is ignored"
                    .to_owned(),
            );
        }
        messages
    }
}

/// `http://host[:port]`, or `None` if the host is not a plain host name or address.
fn proxy_uri(server: Option<&ProxyServer>) -> Option<String> {
    let server = server?;
    let host = server.host.trim();
    if host.is_empty()
        || host
            .chars()
            .any(|character| character.is_whitespace() || "/@?#\\".contains(character))
    {
        return None;
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Some(match server.port {
        Some(port) => format!("http://{host}:{port}"),
        None => format!("http://{host}"),
    })
}

/// One macOS bypass entry in `hyper-util`'s syntax: `Ok(None)` for an empty entry, `Err` for
/// a wildcard that cannot be expressed.
fn bypass_entry(entry: &str) -> Result<Option<String>, ()> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Ok(None);
    }
    if entry == "*" {
        return Ok(Some(entry.to_owned()));
    }
    // `*.example.com`: the domain and its subdomains.
    if let Some(domain) = entry.strip_prefix("*.")
        && !domain.contains('*')
    {
        return Ok(Some(format!(".{domain}")));
    }
    if let Some(network) = ipv4_pattern_to_network(entry) {
        return Ok(Some(network));
    }
    if entry.contains('*') {
        return Err(());
    }
    Ok(Some(entry.to_owned()))
}

/// `10.*`, `192.168.1.*` (trailing wildcards) and `169.254/16` (a shortened network, as in
/// macOS's default list) as a full IPv4 network: `10.0.0.0/8`.
fn ipv4_pattern_to_network(entry: &str) -> Option<String> {
    let (address, prefix) = match entry.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix.parse::<u8>().ok()?)),
        None => (entry, None),
    };
    let parts = address.split('.').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let numbers = parts
        .iter()
        .take_while(|part| **part != "*")
        .map(|part| part.parse::<u8>().ok())
        .collect::<Option<Vec<_>>>()?;
    let wildcards = parts.len() - numbers.len();
    if wildcards > 0 && parts[numbers.len()..].iter().any(|part| *part != "*") {
        return None;
    }
    let prefix = match (prefix, wildcards) {
        (Some(prefix), 0) if prefix <= 32 && numbers.len() < 4 => prefix,
        (None, wildcards) if wildcards > 0 && !numbers.is_empty() => (numbers.len() * 8) as u8,
        _ => return None,
    };
    let mut octets = [0u8; 4];
    octets[..numbers.len()].copy_from_slice(&numbers);
    Some(format!(
        "{}.{}.{}.{}/{prefix}",
        octets[0], octets[1], octets[2], octets[3]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_proxies(exceptions: &[&str]) -> SystemProxySettings {
        SystemProxySettings {
            http: Some(ProxyServer::new("proxy.example", Some(3128))),
            https: Some(ProxyServer::new("secure.example", Some(8443))),
            exceptions: exceptions.iter().map(|entry| entry.to_string()).collect(),
            ..SystemProxySettings::default()
        }
    }

    #[test]
    fn no_proxy_in_use_leaves_every_preference_empty() {
        assert_eq!(
            SystemProxySettings::default().preferences(),
            ProxyPreferences::default()
        );
        let only_exceptions = SystemProxySettings {
            exceptions: vec!["*.local".to_owned()],
            exclude_simple_hostnames: true,
            ..SystemProxySettings::default()
        };
        assert_eq!(only_exceptions.preferences(), ProxyPreferences::default());
    }

    #[test]
    fn both_web_proxies_become_http_proxy_addresses() {
        let preferences = with_proxies(&[]).preferences();

        assert_eq!(preferences.http_uri, "http://proxy.example:3128");
        // The secure web proxy is spoken to in plain HTTP; it tunnels with CONNECT.
        assert_eq!(preferences.https_uri, "http://secure.example:8443");
    }

    #[test]
    fn a_disabled_proxy_stays_empty_while_the_other_is_used() {
        let settings = SystemProxySettings {
            https: None,
            ..with_proxies(&[])
        };

        let preferences = settings.preferences();

        assert_eq!(preferences.http_uri, "http://proxy.example:3128");
        assert_eq!(preferences.https_uri, "");
        assert_ne!(preferences.no_proxy, "");
    }

    #[test]
    fn a_proxy_without_a_port_uses_the_default_port() {
        let settings = SystemProxySettings {
            http: Some(ProxyServer::new("proxy.example", None)),
            ..SystemProxySettings::default()
        };

        assert_eq!(settings.preferences().http_uri, "http://proxy.example");
    }

    #[test]
    fn an_ipv6_proxy_is_bracketed() {
        let settings = SystemProxySettings {
            http: Some(ProxyServer::new("fd00::1", Some(8080))),
            https: Some(ProxyServer::new("[fd00::2]", Some(8080))),
            ..SystemProxySettings::default()
        };

        let preferences = settings.preferences();

        assert_eq!(preferences.http_uri, "http://[fd00::1]:8080");
        assert_eq!(preferences.https_uri, "http://[fd00::2]:8080");
    }

    #[test]
    fn a_host_that_is_not_a_host_name_is_not_used() {
        for host in ["", "  ", "user@proxy.example", "proxy.example/path", "a b"] {
            let settings = SystemProxySettings {
                http: Some(ProxyServer::new(host, Some(8080))),
                ..SystemProxySettings::default()
            };

            assert_eq!(
                settings.preferences(),
                ProxyPreferences::default(),
                "{host:?}"
            );
            assert_eq!(settings.unsupported().len(), 1, "{host:?}");
        }
    }

    #[test]
    fn surrounding_spaces_in_a_host_are_ignored() {
        let settings = SystemProxySettings {
            http: Some(ProxyServer::new(" proxy.example ", Some(80))),
            ..SystemProxySettings::default()
        };

        assert_eq!(settings.preferences().http_uri, "http://proxy.example:80");
    }

    #[test]
    fn loopback_always_bypasses_a_system_proxy() {
        assert_eq!(
            with_proxies(&[]).preferences().no_proxy,
            "localhost,127.0.0.0/8,::1"
        );
    }

    #[test]
    fn the_bypass_list_is_translated_to_servos_syntax() {
        let preferences = with_proxies(&[
            "*.local",
            "169.254/16",
            "10.*",
            "192.168.1.*",
            "example.com",
            ".example.org",
            " intranet ",
            "",
            "172.16.0.0/12",
            "fe80::/10",
            "*",
        ])
        .preferences();

        assert_eq!(
            preferences.no_proxy,
            "localhost,127.0.0.0/8,::1,.local,169.254.0.0/16,10.0.0.0/8,192.168.1.0/24,\
             example.com,.example.org,intranet,172.16.0.0/12,fe80::/10,*"
        );
    }

    #[test]
    fn bypass_entries_servo_cannot_express_are_dropped_and_reported() {
        let settings = with_proxies(&["www.*.example", "*example.com", "10.*.1.*", "ok.example"]);

        assert_eq!(
            settings.preferences().no_proxy,
            "localhost,127.0.0.0/8,::1,ok.example"
        );
        let unsupported = settings.unsupported();
        assert_eq!(unsupported.len(), 3, "{unsupported:?}");
        assert!(unsupported[0].contains("www.*.example"), "{unsupported:?}");
    }

    #[test]
    fn settings_servo_cannot_use_are_reported() {
        let settings = SystemProxySettings {
            socks: Some(ProxyServer::new("socks.example", Some(1080))),
            exclude_simple_hostnames: true,
            auto_config_url: Some("http://wpad.example/proxy.pac".to_owned()),
            auto_discovery: true,
            ..with_proxies(&[])
        };

        let unsupported = settings.unsupported();

        assert_eq!(unsupported.len(), 4, "{unsupported:?}");
        for word in ["SOCKS", "simple host names", "proxy.pac", "discovery"] {
            assert!(
                unsupported.iter().any(|message| message.contains(word)),
                "{word}: {unsupported:?}"
            );
        }
        // What can be used still is.
        assert_eq!(settings.preferences().http_uri, "http://proxy.example:3128");
    }

    #[test]
    fn exclude_simple_hostnames_is_not_reported_without_a_proxy() {
        let settings = SystemProxySettings {
            exclude_simple_hostnames: true,
            exceptions: vec!["www.*.example".to_owned()],
            ..SystemProxySettings::default()
        };

        assert_eq!(settings.unsupported(), Vec::<String>::new());
    }

    #[test]
    fn nothing_is_reported_for_settings_servo_can_use() {
        assert_eq!(
            with_proxies(&["*.local", "169.254/16"]).unsupported(),
            Vec::<String>::new()
        );
    }
}
