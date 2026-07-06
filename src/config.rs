use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_scrape_interval_secs")]
    pub scrape_interval_secs: u64,
    #[serde(default = "default_verify_timeout_secs")]
    pub verify_timeout_secs: u64,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_verify_concurrency")]
    pub verify_concurrency: usize,
    /// 初始播源列表: "名称,URL,分类" 用分号分隔多个
    /// 例如: "my-source,https://example.com/tv.m3u,综合;other,https://other.com/list.m3u8,"
    #[serde(default)]
    pub initial_sources: Option<String>,
}

fn default_db_path() -> String {
    "data/iptv.db".to_string()
}
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    5000
}
fn default_scrape_interval_secs() -> u64 {
    3600 // 每小时爬取一次
}
fn default_verify_timeout_secs() -> u64 {
    10
}
fn default_request_timeout_secs() -> u64 {
    30
}
fn default_verify_concurrency() -> usize {
    20
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            host: default_host(),
            port: default_port(),
            scrape_interval_secs: default_scrape_interval_secs(),
            verify_timeout_secs: default_verify_timeout_secs(),
            request_timeout_secs: default_request_timeout_secs(),
            verify_concurrency: default_verify_concurrency(),
            initial_sources: None,
        }
    }
}

impl Config {
    pub fn from_env_or_default() -> Self {
        let mut config = Self::default();
        if let Ok(db) = std::env::var("DB_PATH") {
            config.db_path = db;
        }
        if let Ok(host) = std::env::var("HOST") {
            config.host = host;
        }
        if let Ok(port) = std::env::var("PORT") {
            if let Ok(p) = port.parse() {
                config.port = p;
            }
        }
        if let Ok(interval) = std::env::var("SCRAPE_INTERVAL_SECS") {
            if let Ok(v) = interval.parse() {
                config.scrape_interval_secs = v;
            }
        }
        if let Ok(timeout) = std::env::var("VERIFY_TIMEOUT_SECS") {
            if let Ok(v) = timeout.parse() {
                config.verify_timeout_secs = v;
            }
        }
        if let Ok(timeout) = std::env::var("REQUEST_TIMEOUT_SECS") {
            if let Ok(v) = timeout.parse() {
                config.request_timeout_secs = v;
            }
        }
        if let Ok(concurrency) = std::env::var("VERIFY_CONCURRENCY") {
            if let Ok(v) = concurrency.parse() {
                config.verify_concurrency = v;
            }
        }
        if let Ok(sources) = std::env::var("INITIAL_SOURCES") {
            config.initial_sources = Some(sources);
        }
        config
    }

    /// 解析 INITIAL_SOURCES 为 (name, url, category) 三元组列表
    pub fn parse_initial_sources(&self) -> Vec<(String, String, Option<String>)> {
        let mut result = Vec::new();
        if let Some(ref raw) = self.initial_sources {
            for part in raw.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let fields: Vec<&str> = part.splitn(3, ',').collect();
                let name = fields.first().map(|s| s.trim().to_string()).unwrap_or_default();
                let url = fields.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
                let category = fields.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if !name.is_empty() && !url.is_empty() {
                    result.push((name, url, category));
                }
            }
        }
        result
    }
}
