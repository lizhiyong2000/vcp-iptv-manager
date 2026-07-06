use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

use crate::models::{ApiResponse, CreateSourceRequest, PlaylistSource};
use crate::AppState;

/// 查询所有播源（包括禁用的）
pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<Vec<PlaylistSource>>> {
    match state.db.list_playlist_sources() {
        Ok(sources) => Json(ApiResponse::success(sources)),
        Err(e) => Json(ApiResponse::error(500, format!("查询播源失败: {}", e))),
    }
}

/// 添加新播源
pub async fn add_source(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSourceRequest>,
) -> Json<ApiResponse<PlaylistSource>> {
    // 基本校验
    if req.name.trim().is_empty() {
        return Json(ApiResponse::error(400, "播源名称不能为空"));
    }
    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Json(ApiResponse::error(400, "播源 URL 必须以 http:// 或 https:// 开头"));
    }

    match state
        .db
        .add_playlist_source(req.name.trim(), req.url.trim(), req.category.as_deref())
    {
        Ok(id) => {
            // 查询刚插入的记录
            match state.db.list_playlist_sources() {
                Ok(sources) => {
                    if let Some(source) = sources.into_iter().find(|s| s.id == id) {
                        tracing::info!("新增播源: {} ({})", source.name, source.url);
                        Json(ApiResponse::success(source))
                    } else {
                        Json(ApiResponse::success(PlaylistSource {
                            id,
                            name: req.name,
                            url: req.url,
                            category: req.category,
                            enabled: true,
                            last_count: None,
                            last_status: None,
                            last_fetch_at: None,
                            created_at: chrono::Utc::now().naive_utc(),
                            updated_at: chrono::Utc::now().naive_utc(),
                        }))
                    }
                }
                Err(e) => Json(ApiResponse::error(500, format!("查询播源失败: {}", e))),
            }
        }
        Err(e) => {
            if e.to_string().contains("UNIQUE") {
                Json(ApiResponse::error(409, "该播源 URL 已存在"))
            } else {
                Json(ApiResponse::error(500, format!("添加播源失败: {}", e)))
            }
        }
    }
}

/// 删除播源
pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<String>> {
    match state.db.delete_playlist_source(id) {
        Ok(true) => {
            tracing::info!("删除播源 id={}", id);
            Json(ApiResponse::success("播源已删除".to_string()))
        }
        Ok(false) => Json(ApiResponse::error(404, "播源不存在")),
        Err(e) => Json(ApiResponse::error(500, format!("删除播源失败: {}", e))),
    }
}

/// 切换播源启用/禁用
#[derive(Debug, Deserialize)]
pub struct ToggleSourceRequest {
    pub enabled: bool,
}

pub async fn toggle_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<ToggleSourceRequest>,
) -> Json<ApiResponse<String>> {
    match state.db.toggle_playlist_source(id, req.enabled) {
        Ok(true) => {
            let status = if req.enabled { "启用" } else { "禁用" };
            tracing::info!("{}播源 id={}", status, id);
            Json(ApiResponse::success(format!("播源已{}", status)))
        }
        Ok(false) => Json(ApiResponse::error(404, "播源不存在")),
        Err(e) => Json(ApiResponse::error(500, format!("操作失败: {}", e))),
    }
}

/// 立即拉取指定播源
pub async fn fetch_source_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<String>> {
    // 查询播源信息
    let sources = match state.db.list_playlist_sources() {
        Ok(s) => s,
        Err(e) => return Json(ApiResponse::error(500, format!("查询播源失败: {}", e))),
    };

    let source = match sources.into_iter().find(|s| s.id == id) {
        Some(s) => s,
        None => return Json(ApiResponse::error(404, "播源不存在")),
    };

    if !source.enabled {
        return Json(ApiResponse::error(400, "播源已禁用，无法手动拉取"));
    }

    let source_name = source.name.clone();
    let db = state.db.clone();
    let client = state.client.clone();

    tokio::spawn(async move {
        use crate::scraper::m3u_source::M3uPlaylistFetcher;
        use crate::scraper::Scraper;

        let fetcher = M3uPlaylistFetcher::new(
            source.name.clone(),
            source.url.clone(),
            source.category.clone(),
            client,
        );

        tracing::info!("手动拉取播源: {} ({})", source.name, source.url);
        match fetcher.scrape().await {
            Ok(items) => {
                let count = items.len() as i32;
                if !items.is_empty() {
                    match db.upsert_play_items(&items) {
                        Ok(n) => tracing::info!(
                            "手动拉取播源 [{}] 完成: 解析 {} 个, 新增/更新 {} 个",
                            source.name, count, n
                        ),
                        Err(e) => tracing::error!("保存播放地址失败: {}", e),
                    }
                }
                if let Err(e) = db.update_playlist_source_status(source.id, count, "ok") {
                    tracing::error!("更新播源状态失败: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("手动拉取播源 [{}] 失败: {}", source.name, e);
                let _ = db.update_playlist_source_status(source.id, 0, "error");
            }
        }
    });

    Json(ApiResponse::success(format!(
        "正在后台拉取播源: {}",
        source_name
    )))
}
