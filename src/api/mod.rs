pub mod channels;
pub mod playitems;
pub mod pull;
pub mod pull_tasks;
pub mod pull_validate;
pub mod sources;

use axum::Router;
use std::sync::Arc;

use super::AppState;

/// 组装所有 API 路由
pub fn build_router() -> Router<Arc<AppState>> {
    Router::new()
        // 播放地址相关
        .route("/api/playitems", axum::routing::get(playitems::list_playitems))
        .route(
            "/api/playitems/export",
            axum::routing::get(playitems::export_m3u8),
        )
        .route(
            "/api/playitems/sources",
            axum::routing::get(playitems::list_sources),
        )
        // 频道相关
        .route("/api/channels", axum::routing::get(channels::list_channels))
        .route(
            "/api/channels/sources",
            axum::routing::get(channels::list_channel_sources),
        )
        .route(
            "/api/channels/{id}/playitems",
            axum::routing::get(channels::get_channel_playitems),
        )
        // 统计
        .route("/api/stats", axum::routing::get(playitems::get_stats))
        // 手动触发爬取+验证
        .route("/api/scrape", axum::routing::post(playitems::trigger_scrape))
        // 手动触发仅验证（不拉取播源）
        .route("/api/verify", axum::routing::post(playitems::trigger_verify))
        // 播源管理
        .route("/api/sources", axum::routing::get(sources::list_sources))
        .route("/api/sources", axum::routing::post(sources::add_source))
        .route(
            "/api/sources/{id}",
            axum::routing::delete(sources::delete_source),
        )
        .route(
            "/api/sources/{id}/toggle",
            axum::routing::post(sources::toggle_source),
        )
        .route(
            "/api/sources/{id}/fetch",
            axum::routing::post(sources::fetch_source_now),
        )
        // 拉流验证任务中心
        .route("/api/pull/tasks", axum::routing::post(pull_tasks::create_pull_tasks))
        .route("/api/pull/tasks", axum::routing::get(pull_tasks::list_pull_tasks))
        .route(
            "/api/pull/tasks/{id}",
            axum::routing::get(pull_tasks::get_pull_task),
        )
        .route(
            "/api/pull/tasks/{id}/stop",
            axum::routing::post(pull_tasks::stop_pull_task),
        )
        .route(
            "/api/pull/tasks/{id}/retry",
            axum::routing::post(pull_tasks::retry_pull_task),
        )
        // 拉流代理（直连 vcp-media-server）
        .route("/api/pull/rtmp", axum::routing::post(pull::pull_rtmp))
        .route("/api/pull/rtsp", axum::routing::post(pull::pull_rtsp))
        .route("/api/pull/hls", axum::routing::post(pull::pull_hls))
        .route("/api/pull/flv", axum::routing::post(pull::pull_flv))
        // 拉流验证（批量提交 RTMP/RTSP/HLS/FLV 拉流任务）
        .route(
            "/api/pull/validate",
            axum::routing::post(pull_validate::pull_validate),
        )
        // 截图图片代理（直连 vcp-media-server）
        .route(
            "/api/snapshot-image/{snapshot_id}",
            axum::routing::get(pull::get_snapshot_image),
        )
}
