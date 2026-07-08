use chrono::{DateTime, Local, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};

// ---- 时间序列化辅助函数：将 UTC NaiveDateTime 转为本地时间字符串 ----

/// 序列化 `NaiveDateTime`（解读为 UTC）为本地时间字符串
fn serialize_dt_local<S: Serializer>(dt: &NaiveDateTime, s: S) -> Result<S::Ok, S::Error> {
    let utc_dt: DateTime<Utc> = DateTime::from_naive_utc_and_offset(*dt, Utc);
    let local = utc_dt.with_timezone(&Local);
    s.serialize_str(&local.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// 序列化 `Option<NaiveDateTime>`（解读为 UTC）为本地时间字符串
fn serialize_opt_dt_local<S: Serializer>(dt: &Option<NaiveDateTime>, s: S) -> Result<S::Ok, S::Error> {
    match dt {
        Some(dt) => serialize_dt_local(dt, s),
        None => s.serialize_none(),
    }
}

/// 频道模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub category: Option<String>,
    pub logo_url: Option<String>,
    #[serde(serialize_with = "serialize_dt_local")]
    pub created_at: NaiveDateTime,
    #[serde(serialize_with = "serialize_dt_local")]
    pub updated_at: NaiveDateTime,
}

/// 播放地址模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayItem {
    pub id: i64,
    pub channel_name: String,
    pub url: String,
    pub source: String,
    pub category: Option<String>,
    pub is_valid: bool,
    pub fail_count: i32,
    #[serde(serialize_with = "serialize_opt_dt_local")]
    pub last_checked: Option<NaiveDateTime>,
    pub resolution: Option<String>,
    pub bitrate: Option<i64>,
    #[serde(serialize_with = "serialize_dt_local")]
    pub created_at: NaiveDateTime,
    #[serde(serialize_with = "serialize_dt_local")]
    pub updated_at: NaiveDateTime,
}

/// 爬取的原始 M3U8 条目
#[derive(Debug, Clone)]
pub struct RawPlayItem {
    pub channel_name: String,
    pub url: String,
    pub source: String,
    pub category: Option<String>,
    pub resolution: Option<String>,
}

/// 播源模型 — 一个 M3U/M3U8 播放列表的 URL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistSource {
    pub id: i64,
    /// 播源名称（便于识别）
    pub name: String,
    /// M3U/M3U8 播放列表的 URL
    pub url: String,
    /// 来源分类标签
    pub category: Option<String>,
    /// 是否启用
    pub enabled: bool,
    /// 上次拉取条目数
    pub last_count: Option<i32>,
    /// 上次拉取状态：ok / error
    pub last_status: Option<String>,
    /// 上次拉取时间
    #[serde(serialize_with = "serialize_opt_dt_local")]
    pub last_fetch_at: Option<NaiveDateTime>,
    #[serde(serialize_with = "serialize_dt_local")]
    pub created_at: NaiveDateTime,
    #[serde(serialize_with = "serialize_dt_local")]
    pub updated_at: NaiveDateTime,
}

/// 创建播源的请求体
#[derive(Debug, Clone, Deserialize)]
pub struct CreateSourceRequest {
    pub name: String,
    pub url: String,
    pub category: Option<String>,
}

/// API: 分页响应
#[derive(Debug, Clone, Serialize)]
pub struct PageResponse<T: Serialize> {
    pub total: i64,
    pub page_num: i32,
    pub page_size: i32,
    pub items: Vec<T>,
}

/// API: 统计信息
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub total_channels: i64,
    pub total_play_items: i64,
    pub valid_play_items: i64,
    pub invalid_play_items: i64,
    pub total_sources: i64,
    pub active_sources: i64,
    pub sources: Vec<SourceStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceStats {
    pub name: String,
    pub total: i64,
    pub valid: i64,
}

/// 拉流验证任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullTask {
    pub id: i64,
    pub channel_name: Option<String>,
    pub play_item_id: Option<i64>,
    pub url: String,
    pub stream_id: String,
    pub protocol: String,
    pub status: String,
    pub error_message: Option<String>,
    pub snapshot_id: Option<String>,
    pub snapshot_status: Option<String>,
    pub retry_count: i32,
    #[serde(serialize_with = "serialize_dt_local")]
    pub created_at: NaiveDateTime,
    #[serde(serialize_with = "serialize_dt_local")]
    pub updated_at: NaiveDateTime,
    #[serde(serialize_with = "serialize_opt_dt_local")]
    pub started_at: Option<NaiveDateTime>,
    #[serde(serialize_with = "serialize_opt_dt_local")]
    pub completed_at: Option<NaiveDateTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePullTaskRequest {
    pub url: String,
    pub stream_id: String,
    pub protocol: Option<String>,
    pub channel_name: Option<String>,
    pub play_item_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullTaskListResponse {
    pub total: i64,
    pub running: i64,
    pub items: Vec<PullTask>,
}

/// API: 通用响应
#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            code: 0,
            message: "ok".to_string(),
            data: Some(data),
        }
    }
}

impl<T: Serialize> ApiResponse<T> {
    pub fn error(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}
