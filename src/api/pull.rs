use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing;

use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequest {
    pub url: String,
    pub stream_id: String,
}

pub fn detect_protocol(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    if lower.starts_with("rtmp://") || lower.starts_with("rtmps://") {
        "rtmp"
    } else if lower.starts_with("rtsp://") || lower.starts_with("rtsps://") {
        "rtsp"
    } else if lower.ends_with(".m3u8") || lower.contains(".m3u8?") {
        "hls"
    } else if lower.ends_with(".flv") || lower.contains(".flv?") {
        "flv"
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        "flv"
    } else {
        "unknown"
    }
}

pub fn media_server_pull_path(protocol: &str) -> Option<&'static str> {
    match protocol {
        "rtmp" => Some("/api/pull/rtmp"),
        "rtsp" => Some("/api/pull/rtsp"),
        "hls" => Some("/api/pull/hls"),
        "flv" => Some("/api/pull/flv"),
        _ => None,
    }
}

pub async fn submit_pull(
    client: &reqwest::Client,
    media_server_url: &str,
    protocol: &str,
    url: &str,
    stream_id: &str,
) -> Result<(StatusCode, Value), String> {
    let Some(path) = media_server_pull_path(protocol) else {
        return Err(format!("不支持的协议: {protocol}"));
    };

    let target_url = format!(
        "{}{}",
        media_server_url.trim_end_matches('/'),
        path
    );
    let pull_body = json!({
        "url": url,
        "stream_id": stream_id,
    });

    let resp = client
        .post(&target_url)
        .json(&pull_body)
        .send()
        .await
        .map_err(|e| format!("请求 media-server 失败 ({target_url}): {e}"))?;

    let status = resp.status();
    let body_json = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("解析 media-server 返回失败 ({target_url}): {e}"))?;

    Ok((status, body_json))
}

fn pull_response(result: Result<(StatusCode, Value), String>) -> Response {
    match result {
        Ok((status, body)) => (status, Json(body)).into_response(),
        Err(message) => {
            tracing::warn!("拉流代理失败: {}", message);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": message })),
            )
                .into_response()
        }
    }
}

async fn proxy_pull(
    state: &AppState,
    protocol: &str,
    body: PullRequest,
) -> Result<(StatusCode, Value), String> {
    submit_pull(
        &state.client,
        &state.media_server_url,
        protocol,
        &body.url,
        &body.stream_id,
    )
    .await
}

pub async fn pull_rtmp(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullRequest>,
) -> Response {
    pull_response(proxy_pull(&state, "rtmp", body).await)
}

pub async fn pull_rtsp(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullRequest>,
) -> Response {
    pull_response(proxy_pull(&state, "rtsp", body).await)
}

pub async fn pull_hls(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullRequest>,
) -> Response {
    pull_response(proxy_pull(&state, "hls", body).await)
}

pub async fn pull_flv(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullRequest>,
) -> Response {
    pull_response(proxy_pull(&state, "flv", body).await)
}

pub fn url_encode_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", byte);
            }
        }
    }
    out
}

pub async fn delete_stream(
    client: &reqwest::Client,
    media_server_url: &str,
    stream_id: &str,
) -> Result<(), String> {
    let target_url = format!(
        "{}/api/stream/{}",
        media_server_url.trim_end_matches('/'),
        url_encode_path_segment(stream_id)
    );
    let resp = client
        .delete(&target_url)
        .send()
        .await
        .map_err(|e| format!("删除流失败 ({target_url}): {e}"))?;
    if resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "删除流失败 ({target_url}): HTTP {}",
            resp.status()
        ))
    }
}

pub async fn stream_has_keyframe(
    client: &reqwest::Client,
    media_server_url: &str,
    stream_id: &str,
) -> Result<bool, String> {
    let target_url = format!(
        "{}/api/metrics/{}",
        media_server_url.trim_end_matches('/'),
        url_encode_path_segment(stream_id)
    );
    let resp = client
        .get(&target_url)
        .send()
        .await
        .map_err(|e| format!("读取指标失败 ({target_url}): {e}"))?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    let body = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("解析指标失败 ({target_url}): {e}"))?;
    let latest_idr = body
        .pointer("/ring/latest_idr_seq")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let latest_seq = body
        .pointer("/ring/latest_seq")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(latest_idr > 0 || latest_seq > 0)
}

pub async fn wait_for_stream_keyframe(
    client: &reqwest::Client,
    media_server_url: &str,
    stream_id: &str,
    timeout: std::time::Duration,
    poll: std::time::Duration,
    should_stop: impl Fn() -> bool,
) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if should_stop() {
            return Err("任务已停止".to_string());
        }
        if stream_has_keyframe(client, media_server_url, stream_id).await? {
            return Ok(true);
        }
        tokio::time::sleep(poll).await;
    }
    Ok(false)
}

pub async fn submit_snapshot(
    client: &reqwest::Client,
    media_server_url: &str,
    stream_id: &str,
) -> Result<(StatusCode, Value), String> {
    let target_url = format!("{}/api/snapshot", media_server_url.trim_end_matches('/'));
    let body = json!({ "stream_id": stream_id });
    let resp = client
        .post(&target_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("提交截图失败 ({target_url}): {e}"))?;
    let status = resp.status();
    let value = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("解析截图返回失败 ({target_url}): {e}"))?;
    Ok((status, value))
}

pub async fn get_snapshot_entry(
    client: &reqwest::Client,
    media_server_url: &str,
    snapshot_id: &str,
) -> Result<(StatusCode, Value), String> {
    let target_url = format!(
        "{}/api/snapshots/{}",
        media_server_url.trim_end_matches('/'),
        url_encode_path_segment(snapshot_id)
    );
    let resp = client
        .get(&target_url)
        .send()
        .await
        .map_err(|e| format!("查询截图失败 ({target_url}): {e}"))?;
    let status = resp.status();
    let value = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("解析截图状态失败 ({target_url}): {e}"))?;
    Ok((status, value))
}

pub async fn wait_for_snapshot(
    client: &reqwest::Client,
    media_server_url: &str,
    snapshot_id: &str,
    timeout: std::time::Duration,
    poll: std::time::Duration,
    should_stop: impl Fn() -> bool,
) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if should_stop() {
            return Err("任务已停止".to_string());
        }
        let (_, body) = get_snapshot_entry(client, media_server_url, snapshot_id).await?;
        let snapshot = body.get("snapshot").cloned().unwrap_or(body);
        let status = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match status {
            "completed" => return Ok(snapshot),
            "error" | "failed" => {
                let err = snapshot
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("截图失败");
                return Err(err.to_string());
            }
            _ => {
                tokio::time::sleep(poll).await;
            }
        }
    }
    Err("等待截图超时".to_string())
}

pub async fn get_snapshot_image_bytes(
    client: &reqwest::Client,
    media_server_url: &str,
    snapshot_id: &str,
) -> Result<(Vec<u8>, String), (StatusCode, String)> {
    let target_url = format!(
        "{}/api/snapshots/{}.jpg",
        media_server_url.trim_end_matches('/'),
        url_encode_path_segment(snapshot_id)
    );
    let resp = client.get(&target_url).send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("请求 media-server 失败 ({target_url}): {e}"),
        )
    })?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp.bytes().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("读取截图数据失败 ({target_url}): {e}"),
        )
    })?;

    if !status.is_success() {
        return Err((
            status,
            String::from_utf8_lossy(&bytes).to_string(),
        ));
    }

    Ok((bytes.to_vec(), content_type))
}

pub async fn get_snapshot_image(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Response {
    match get_snapshot_image_bytes(&state.client, &state.media_server_url, &snapshot_id).await {
        Ok((bytes, content_type)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type)],
            bytes,
        )
            .into_response(),
        Err((status, message)) => {
            tracing::warn!("截图代理失败 id='{snapshot_id}': {message}");
            (
                status,
                [(header::CONTENT_TYPE, "application/json".to_string())],
                Json(json!({ "error": message })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_protocols_to_media_server_paths() {
        assert_eq!(media_server_pull_path("rtmp"), Some("/api/pull/rtmp"));
        assert_eq!(media_server_pull_path("rtsp"), Some("/api/pull/rtsp"));
        assert_eq!(media_server_pull_path("hls"), Some("/api/pull/hls"));
        assert_eq!(media_server_pull_path("flv"), Some("/api/pull/flv"));
    }

    #[test]
    fn detects_protocol_from_url() {
        assert_eq!(detect_protocol("rtmp://127.0.0.1/live/a"), "rtmp");
        assert_eq!(detect_protocol("https://example.com/live.m3u8"), "hls");
        assert_eq!(detect_protocol("http://example.com/live.flv"), "flv");
    }
}
