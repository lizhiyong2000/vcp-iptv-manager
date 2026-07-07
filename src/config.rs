use serde::Deserialize;

/// 播源配置项
#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub category: Option<String>,
}

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
    /// 初始播源列表
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    /// vcp-media-manager 地址（用于转发拉流验证任务）
    #[serde(default = "default_media_manager_url")]
    pub media_manager_url: String,
}

fn default_media_manager_url() -> String {
    "http://127.0.0.1:8090".to_string()
}

fn default_db_path() -> String {
    "data/iptv.db".to_string()
}
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    5001
}
fn default_scrape_interval_secs() -> u64 {
    3600
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
            sources: Vec::new(),
            media_manager_url: default_media_manager_url(),
        }
    }
}

impl Config {
    /// 从配置文件加载，文件不存在时使用默认值
    pub fn from_file_or_default() -> Self {
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());

        let mut config = match std::fs::read_to_string(&config_path) {
            Ok(content) => {
                match toml::from_str::<Config>(&content) {
                    Ok(c) => {
                        tracing::info!("已加载配置文件: {}", config_path);
                        c
                    }
                    Err(e) => {
                        tracing::warn!("配置文件解析失败 ({}): {}, 使用默认配置", config_path, e);
                        Config::default()
                    }
                }
            }
            Err(_) => {
                tracing::info!("未找到配置文件 ({}), 使用默认配置", config_path);
                Config::default()
            }
        };

        // 环境变量覆盖（便于容器化部署和临时调整）
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
        // 兼容旧 INITIAL_SOURCES 环境变量（分号分隔格式）
        if let Ok(raw) = std::env::var("INITIAL_SOURCES") {
            let parsed = Self::parse_legacy_sources(&raw);
            if !parsed.is_empty() {
                config.sources = parsed;
            }
        }

        config
    }

    /// 旧格式兼容: "name,url,category;name2,url2,cat2"
    fn parse_legacy_sources(raw: &str) -> Vec<SourceConfig> {
        let mut result = Vec::new();
        for part in raw.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let fields: Vec<&str> = part.splitn(3, ',').collect();
            let name = fields.first().map(|s| s.trim().to_string()).unwrap_or_default();
            let url = fields.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
            let category = fields
                .get(2)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if !name.is_empty() && !url.is_empty() {
                result.push(SourceConfig { name, url, category });
            }
        }
        result
    }

    /// 转为 db::ensure_playlist_sources 接受的格式
    pub fn parse_initial_sources(&self) -> Vec<(String, String, Option<String>)> {
        self.sources
            .iter()
            .map(|s| (s.name.clone(), s.url.clone(), s.category.clone()))
            .collect()
    }
}
