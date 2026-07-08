use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::pull::detect_protocol;
use crate::models::{ApiResponse, CreatePullTaskRequest, PullTask};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateTask {
    pub url: String,
    pub stream_id: String,
    pub protocol: Option<String>,
    pub channel_name: Option<String>,
    pub play_item_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateRequest {
    pub tasks: Vec<PullValidateTask>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullValidateResult {
    pub task_id: i64,
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
    pub tasks: Vec<PullTask>,
    pub total: usize,
    pub queued: usize,
    pub error: usize,
}

pub async fn pull_validate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PullValidateRequest>,
) -> Json<ApiResponse<PullValidateResponse>> {
    let mut create_requests = Vec::new();
    let mut invalid_results = Vec::new();

    for task in &body.tasks {
        let protocol = task
            .protocol
            .as_deref()
            .unwrap_or_else(|| detect_protocol(&task.url))
            .to_string();
        if crate::api::pull::media_server_pull_path(&protocol).is_none() {
            invalid_results.push(PullValidateResult {
                task_id: 0,
                stream_id: task.stream_id.clone(),
                url: task.url.clone(),
                protocol: protocol.clone(),
                status: "error".to_string(),
                message: format!("不支持的协议: {protocol}"),
            });
            continue;
        }
        create_requests.push(CreatePullTaskRequest {
            url: task.url.clone(),
            stream_id: task.stream_id.clone(),
            protocol: Some(protocol),
            channel_name: task.channel_name.clone(),
            play_item_id: task.play_item_id,
        });
    }

    let mut created_tasks = Vec::new();
    if !create_requests.is_empty() {
        match state.task_center.create_tasks(create_requests).await {
            Ok(tasks) => created_tasks = tasks,
            Err(err) => {
                return Json(ApiResponse::error(400, err.to_string()));
            }
        }
    }

    let mut results = invalid_results;
    for task in created_tasks.iter() {
        results.push(PullValidateResult {
            task_id: task.id,
            stream_id: task.stream_id.clone(),
            url: task.url.clone(),
            protocol: task.protocol.clone(),
            status: task.status.clone(),
            message: "已加入拉流验证任务队列".to_string(),
        });
    }

    let error = results.iter().filter(|r| r.status == "error").count();
    let queued = results.len().saturating_sub(error);

    Json(ApiResponse::success(PullValidateResponse {
        total: body.tasks.len(),
        queued,
        error,
        results,
        tasks: created_tasks,
    }))
}
