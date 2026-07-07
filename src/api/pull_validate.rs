use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing;

use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateTask {
    pub url: String,
    pub stream_id: String,
    pub protocol: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateRequest {
    pub tasks: Vec<PullValidateTask>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateResult {
    pub stream_id: String,
    pub url: String,
    pub protocol: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateResponse {
    pub results: Vec<PullValidateResult>,
    pub total: usize,
    pub success: usize,
    pub error: usize,
}

fn detect_protocol(url: &str) -> &str {
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
        // Default HTTP URLs: try FLV first (streaming), fallback to HLS
        "flv"
    } else {
        "unknown"
    }
}

pub async fn pull_validate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullValidateRequest>,
) -> Json<ApiResponse<PullValidateResponse>> {
    let mut results = Vec::new();
    let mut success_count = 0;
    let mut error_count = 0;

    for task in &body.tasks {
        let protocol = task
            .protocol
            .as_deref()
            .unwrap_or_else(|| detect_protocol(&task.url));

        let (pull_path, protocol_label) = match protocol {
            "rtmp" => ("/api/pull/rtmp", "rtmp"),
            "rtsp" => ("/api/pull/rtsp", "rtsp"),
            "hls" => ("/api/pull/hls", "hls"),
            "flv" => ("/api/pull/flv", "flv"),
            _ => {
                results.push(PullValidateResult {
                    stream_id: task.stream_id.clone(),
                    url: task.url.clone(),
                    protocol: protocol.to_string(),
                    status: "error".to_string(),
                    message: format!(
                        "不支持的协议: {}. 仅支持 RTMP/RTSP/HLS/FLV",
                        protocol
                    ),
                });
                error_count += 1;
                continue;
            }
        };

        let target_url = format!("{}{}", state.media_manager_url, pull_path);
        let pull_body = json!({
            "url": task.url,
            "stream_id": task.stream_id,
        });

        match state
            .client
            .post(&target_url)
            .json(&pull_body)
            .send()
            .await
        {
            Ok(resp) => {
                let status_code = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(body_json) => {
                        if status_code.is_success() {
                            let msg = body_json
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("拉流成功")
                                .to_string();
                            results.push(PullValidateResult {
                                stream_id: task.stream_id.clone(),
                                url: task.url.clone(),
                                protocol: protocol_label.to_string(),
                                status: "success".to_string(),
                                message: msg,
                            });
                            success_count += 1;
                        } else {
                            let err = body_json
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("拉流失败")
                                .to_string();
                            results.push(PullValidateResult {
                                stream_id: task.stream_id.clone(),
                                url: task.url.clone(),
                                protocol: protocol_label.to_string(),
                                status: "error".to_string(),
                                message: err,
                            });
                            error_count += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "解析 media-manager 返回失败 ({}): {}",
                            target_url,
                            e
                        );
                        results.push(PullValidateResult {
                            stream_id: task.stream_id.clone(),
                            url: task.url.clone(),
                            protocol: protocol_label.to_string(),
                            status: "error".to_string(),
                            message: format!("解析返回失败: {}", e),
                        });
                        error_count += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("请求 media-manager 失败 ({}): {}", target_url, e);
                results.push(PullValidateResult {
                    stream_id: task.stream_id.clone(),
                    url: task.url.clone(),
                    protocol: protocol_label.to_string(),
                    status: "error".to_string(),
                    message: format!("请求 media-manager 失败: {}", e),
                });
                error_count += 1;
            }
        }
    }

    Json(ApiResponse::success(PullValidateResponse {
        results,
        total: body.tasks.len(),
        success: success_count,
        error: error_count,
    }))
}
