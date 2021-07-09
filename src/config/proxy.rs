use http::uri::{InvalidUri, Uri};
use hyper_proxy::{Custom, Intercept, Proxy, ProxyConnector};
use std::collections::HashSet;

fn from_env(key: &str) -> Option<String> {
    std::env::var(key.to_string())
        .ok()
        .or_else(|| std::env::var(key.to_lowercase()).ok())
}

struct NoProxyCache(HashSet<Uri>);

impl From<&HashSet<String>> for NoProxyCache {
    fn from(hashset: &HashSet<String>) -> Self {
        Self(
            hashset
                .iter()
                .filter_map(|uri| uri.parse::<Uri>().ok())
                .collect(),
        )
    }
}

impl NoProxyCache {
    fn matches(&self, scheme: Option<&str>, host: Option<&str>, port: Option<u16>) -> bool {
        for uri in self.0.iter() {
            if uri.scheme_str() == scheme && uri.host() == host && uri.port_u16() == port {
                return true;
            }
        }
        false
    }

    fn intercept(self, expected_scheme: &'static str) -> Intercept {
        Intercept::Custom(Custom::from(
            move |scheme: Option<&str>, host: Option<&str>, port: Option<u16>| {
                if scheme != Some(expected_scheme) {
                    return false;
                }
                // only intercapt those that don't match
                !self.matches(scheme, host, port)
            },
        ))
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Default, Debug, PartialEq)]
pub struct ProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: HashSet<String>,
}

impl ProxyConfig {
    pub fn from_env() -> Self {
        Self {
            http: from_env("HTTP_PROXY"),
            https: from_env("HTTP_PROXYS"),
            no_proxy: from_env("NO_PROXY")
                .map(|value| {
                    value
                        .split(",")
                        .map(|item| item.trim().to_string())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn merge(&self, other: &Self) -> Self {
        Self {
            http: self.http.clone().or(other.http.clone()),
            https: self.https.clone().or(other.https.clone()),
            no_proxy: self
                .no_proxy
                .union(&other.no_proxy)
                .map(ToString::to_string)
                .collect(),
        }
    }

    pub fn maybe_merge(&self, other: &Option<Self>) -> Self {
        if let Some(other) = other {
            self.merge(other)
        } else {
            self.clone()
        }
    }

    fn http_intercept(&self) -> Intercept {
        NoProxyCache::from(&self.no_proxy).intercept("http")
    }

    fn http_proxy(&self) -> Result<Option<Proxy>, InvalidUri> {
        if let Some(ref url) = self.http {
            Ok(Some(Proxy::new(self.http_intercept(), url.parse()?)))
        } else {
            Ok(None)
        }
    }

    fn https_intercept(&self) -> Intercept {
        NoProxyCache::from(&self.no_proxy).intercept("https")
    }

    fn https_proxy(&self) -> Result<Option<Proxy>, InvalidUri> {
        if let Some(ref url) = self.https {
            Ok(Some(Proxy::new(self.https_intercept(), url.parse()?)))
        } else {
            Ok(None)
        }
    }

    pub fn configure<C>(&self, connector: &mut ProxyConnector<C>) -> Result<(), InvalidUri> {
        if let Some(proxy) = self.http_proxy()? {
            connector.add_proxy(proxy);
        }
        if let Some(proxy) = self.https_proxy()? {
            connector.add_proxy(proxy);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_cache() {
        let cache = NoProxyCache::from(&{
            let mut set = HashSet::new();
            set.insert("127.0.0.1".into());
            set.insert("https://www.google.com".into());
            set.insert("http://rick.sanchez:8080".into());
            set
        });
        assert!(cache.matches(None, Some("127.0.0.1"), None));
        assert!(!cache.matches(None, Some("www.google.com"), None));
        assert!(!cache.matches(Some("http"), Some("localhost"), None));
        assert!(cache.matches(Some("http"), Some("rick.sanchez"), Some(8080)));
    }

    #[test]
    fn merge() {
        let first = ProxyConfig {
            http: Some("http://1.2.3.4:5678".into()),
            https: None,
            no_proxy: {
                let mut set = HashSet::new();
                set.insert("127.0.0.1".into());
                set.insert("http://google.com".into());
                set
            },
        };
        let second = ProxyConfig {
            http: Some("http://1.2.3.5:5678".into()),
            https: Some("https://2.3.4.5:9876".into()),
            no_proxy: {
                let mut set = HashSet::new();
                set.insert("localhost".into());
                set
            },
        };
        let result = first.merge(&second);
        assert_eq!(result.http, Some("http://1.2.3.4:5678".into()));
        assert_eq!(result.https, Some("https://2.3.4.5:9876".into()));
        assert!(result.no_proxy.contains(&"127.0.0.1".to_string()));
        assert!(result.no_proxy.contains(&"localhost".to_string()));
    }
}
