use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::models::{ApiResponse, CreatePullTaskRequest, PullTask, PullTaskListResponse};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct PullTaskListQuery {
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Deserialize)]
pub struct CreatePullTasksRequest {
    pub tasks: Vec<CreatePullTaskRequest>,
}

pub async fn create_pull_tasks(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreatePullTasksRequest>,
) -> Json<ApiResponse<Vec<PullTask>>> {
    match state.task_center.create_tasks(body.tasks).await {
        Ok(tasks) => Json(ApiResponse::success(tasks)),
        Err(err) => Json(ApiResponse::error(400, err.to_string())),
    }
}

pub async fn list_pull_tasks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PullTaskListQuery>,
) -> Json<ApiResponse<PullTaskListResponse>> {
    let limit = query.limit.clamp(1, 500);
    match state.task_center.list_tasks(query.status.as_deref(), limit) {
        Ok(items) => {
            let running = state.task_center.running_count().unwrap_or(0);
            Json(ApiResponse::success(PullTaskListResponse {
                total: items.len() as i64,
                running,
                items,
            }))
        }
        Err(err) => Json(ApiResponse::error(500, err.to_string())),
    }
}

pub async fn get_pull_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<PullTask>> {
    match state.task_center.get_task(id) {
        Ok(Some(task)) => Json(ApiResponse::success(task)),
        Ok(None) => Json(ApiResponse::error(404, format!("任务不存在: {id}"))),
        Err(err) => Json(ApiResponse::error(500, err.to_string())),
    }
}

pub async fn stop_pull_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<PullTask>> {
    match state.task_center.stop_task(id).await {
        Ok(task) => Json(ApiResponse::success(task)),
        Err(err) => Json(ApiResponse::error(400, err.to_string())),
    }
}

pub async fn retry_pull_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<PullTask>> {
    match state.task_center.retry_task(id).await {
        Ok(task) => Json(ApiResponse::success(task)),
        Err(err) => Json(ApiResponse::error(400, err.to_string())),
    }
}
