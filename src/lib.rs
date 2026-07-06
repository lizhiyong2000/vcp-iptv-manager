pub mod api;
pub mod config;
pub mod db;
pub mod models;
pub mod scraper;
pub mod scheduler;
pub mod verifier;

use std::sync::Arc;

/// 应用全局状态
pub struct AppState {
    pub db: Arc<db::Database>,
    pub verifier: Arc<verifier::Verifier>,
    /// 共享 HTTP 客户端（用于播源拉取等）
    pub client: reqwest::Client,
}
