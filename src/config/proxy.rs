fn from_env(key: &str) -> Option<String> {
    std::env::var(key.to_string())
        .ok()
        .or_else(|| std::env::var(key.to_lowercase()).ok())
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct ProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Vec<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            http: from_env("HTTP_PROXY"),
            https: from_env("HTTP_PROXYS"),
            no_proxy: from_env("NO_PROXY")
                .map(|value| value.split(",").map(ToString::to_string).collect())
                .unwrap_or_default(),
        }
    }
}
