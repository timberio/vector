#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default)]
pub struct ProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Vec<String>,
}
