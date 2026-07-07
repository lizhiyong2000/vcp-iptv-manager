use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

use crate::models::{ApiResponse, Channel, PageResponse, PlayItem};
use crate::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct ChannelQuery {
    pub keyword: Option<String>,
    pub source: Option<String>,
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

/// 分页查询频道列表
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ChannelQuery>,
) -> Json<ApiResponse<PageResponse<Channel>>> {
    match state.db.list_channels(
        params.keyword.as_deref(),
        params.source.as_deref(),
        params.page_num.max(1),
        params.page_size.min(200).max(1),
    ) {
        Ok((items, total)) => Json(ApiResponse::success(PageResponse {
            total,
            page_num: params.page_num.max(1),
            page_size: params.page_size.min(200).max(1),
            items,
        })),
        Err(e) => Json(ApiResponse::error(500, format!("查询频道失败: {}", e))),
    }
}

/// 获取频道来源列表
pub async fn list_channel_sources(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<Vec<String>>> {
    match state.db.get_sources() {
        Ok(sources) => Json(ApiResponse::success(sources)),
        Err(e) => Json(ApiResponse::error(500, format!("查询失败: {}", e))),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ChannelPlayItemQuery {
    #[serde(default = "default_page_num")]
    pub page_num: i32,
    #[serde(default = "default_page_size")]
    pub page_size: i32,
}

/// 查询某频道下的所有播放地址（返回所有来源的播放地址）
pub async fn get_channel_playitems(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<i64>,
    Query(params): Query<ChannelPlayItemQuery>,
) -> Json<ApiResponse<PageResponse<PlayItem>>> {
    // 先查频道
    let channel = match state.db.get_channel(channel_id) {
        Ok(Some(c)) => c,
        Ok(None) => return Json(ApiResponse::error(404, "频道不存在")),
        Err(e) => return Json(ApiResponse::error(500, format!("查询频道失败: {}", e))),
    };

    // 按 channel_name 查询所有来源的播放地址（已合并）
    match state.db.get_channel_playitems(
        &channel.name,
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
