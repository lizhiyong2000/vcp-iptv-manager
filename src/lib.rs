pub mod api;
pub mod config;
pub mod db;
pub mod models;
pub mod scraper;
pub mod scheduler;
pub mod task_center;
pub mod verifier;

use std::sync::Arc;

use task_center::PullTaskCenter;

/// 应用全局状态
pub struct AppState {
    pub db: Arc<db::Database>,
    pub verifier: Arc<verifier::Verifier>,
    /// 共享 HTTP 客户端（用于播源拉取等）
    pub client: reqwest::Client,
    /// vcp-media-server 基础 URL（用于转发拉流任务）
    pub media_server_url: String,
    pub task_center: Arc<PullTaskCenter>,
}
