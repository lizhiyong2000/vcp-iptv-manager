use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

use crate::models::{ApiResponse, PageResponse, PlayItem};
use crate::scraper::m3u_source::M3uPlaylistFetcher;
use crate::scraper::Scraper;
use crate::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct PlayItemQuery {
    pub channel: Option<String>,
    pub source: Option<String>,
    pub is_valid: Option<bool>,
    pub keyword: Option<String>,
    #[serde(default = "default_page_num")]
    pub page_num: i32,
    #[serde(default = "default_page_size")]
    pub page_size: i32,
}

fn default_page_num() -> i32 {
    1
}
fn default_page_size() -> i32 {
    50
}

/// 分页查询播放地址列表
pub async fn list_playitems(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PlayItemQuery>,
) -> Json<ApiResponse<PageResponse<PlayItem>>> {
    match state.db.list_play_items(
        params.channel.as_deref(),
        params.source.as_deref(),
        params.is_valid,
        params.keyword.as_deref(),
        params.page_num.max(1),
        params.page_size.min(200).max(1),
    ) {
        Ok((items, total)) => Json(ApiResponse::success(PageResponse {
            total,
            page_num: params.page_num.max(1),
            page_size: params.page_size.min(200).max(1),
            items,
        })),
        Err(e) => Json(ApiResponse::error(500, format!("查询播放地址失败: {}", e))),
    }
}

/// 导出有效播放地址为 M3U8 格式
pub async fn export_m3u8(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.get_valid_play_items() {
        Ok(items) => {
            let mut m3u8 = String::from("#EXTM3U\n");

            let mut grouped: std::collections::BTreeMap<String, Vec<&PlayItem>> =
                std::collections::BTreeMap::new();
            for item in &items {
                grouped
                    .entry(item.channel_name.clone())
                    .or_default()
                    .push(item);
            }

            for (channel_name, urls) in &grouped {
                for item in urls {
                    m3u8.push_str(&format!(
                        "#EXTINF:-1,{} [{}]\n{}\n",
                        channel_name, item.source, item.url
                    ));
                }
            }

            axum::response::Response::builder()
                .header("Content-Type", "application/vnd.apple.mpegurl")
                .header(
                    "Content-Disposition",
                    "attachment; filename=iptv_playlist.m3u8",
                )
                .body(axum::body::Body::from(m3u8))
                .unwrap()
        }
        Err(e) => axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from(format!("导出失败: {}", e)))
            .unwrap(),
    }
}

/// 获取播放地址的来源列表
pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<Vec<String>>> {
    match state.db.get_sources() {
        Ok(sources) => Json(ApiResponse::success(sources)),
        Err(e) => Json(ApiResponse::error(500, format!("查询失败: {}", e))),
    }
}

/// 获取统计信息
pub async fn get_stats(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<crate::models::Stats>> {
    match state.db.get_stats() {
        Ok(stats) => Json(ApiResponse::success(stats)),
        Err(e) => Json(ApiResponse::error(500, format!("查询统计失败: {}", e))),
    }
}

/// 手动触发仅验证（不拉取播源，仅验证未验证的播放地址）
pub async fn trigger_verify(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<String>> {
    let verifier = state.verifier.clone();

    tokio::spawn(async move {
        tracing::info!("手动触发验证任务");
        match verifier.verify_unchecked().await {
            Ok(result) => {
                tracing::info!(
                    "验证完成: 总数={}, 有效={}, 无效={}",
                    result.total,
                    result.valid,
                    result.invalid,
                );
            }
            Err(e) => {
                tracing::error!("验证失败: {}", e);
            }
        }
    });

    Json(ApiResponse::success(
        "验证任务已触发，正在后台执行".to_string(),
    ))
}

/// 手动触发完整爬取任务（所有启用的播源）
pub async fn trigger_scrape(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<String>> {
    let db = state.db.clone();
    let verifier = state.verifier.clone();
    let client = state.client.clone();

    tokio::spawn(async move {
        tracing::info!("手动触发播源拉取任务");

        // 拉取所有启用的 M3U 播源
        match db.get_enabled_playlist_sources() {
            Ok(sources) => {
                for source in &sources {
                    let fetcher = M3uPlaylistFetcher::new(
                        source.name.clone(),
                        source.url.clone(),
                        source.category.clone(),
                        client.clone(),
                    );
                    let source_name = source.name.clone();
                    match fetcher.scrape().await {
                        Ok(items) => {
                            let count = items.len() as i32;
                            if !items.is_empty() {
                                match db.upsert_play_items(&items) {
                                    Ok(n) => tracing::info!(
                                        "手动拉取-播源 [{}]: 解析 {} 个, 新增/更新 {} 个",
                                        source_name, count, n
                                    ),
                                    Err(e) => tracing::error!(
                                        "手动拉取-播源 [{}]: 保存失败: {}",
                                        source_name, e
                                    ),
                                }
                            }
                            let _ = db.update_playlist_source_status(source.id, count, "ok");
                        }
                        Err(e) => {
                            tracing::warn!("手动拉取-播源 [{}]: 拉取失败: {}", source_name, e);
                            let _ = db.update_playlist_source_status(source.id, 0, "error");
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
            Err(e) => tracing::error!("查询播源列表失败: {}", e),
        }

        // 立即验证新增的地址
        if let Err(e) = verifier.verify_unchecked().await {
            tracing::error!("手动验证失败: {}", e);
        }
    });

    Json(ApiResponse::success(
        "播源拉取任务已触发，正在后台执行".to_string(),
    ))
}
