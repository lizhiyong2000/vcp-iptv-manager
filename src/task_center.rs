use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tracing::{error, info, warn};

use crate::api::pull::{
    delete_stream, detect_protocol, media_server_pull_path, submit_pull, submit_snapshot,
    wait_for_snapshot, wait_for_stream_keyframe,
};
use crate::db::Database;
use crate::models::{CreatePullTaskRequest, PullTask};

pub const MAX_CONCURRENT_PULL_TASKS: usize = 5;
const KEYFRAME_WAIT: Duration = Duration::from_secs(120);
const SNAPSHOT_WAIT: Duration = Duration::from_secs(130);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct PullTaskCenter {
    db: Arc<Database>,
    client: reqwest::Client,
    media_server_url: String,
    slots: Arc<Semaphore>,
    stop_flags: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
    notify: Arc<Notify>,
}

impl PullTaskCenter {
    pub fn new(db: Arc<Database>, client: reqwest::Client, media_server_url: String) -> Self {
        Self {
            db,
            client,
            media_server_url,
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_PULL_TASKS)),
            stop_flags: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn notify(&self) {
        self.notify.notify_waiters();
    }

    pub async fn recover_on_startup(&self) -> Result<usize> {
        let recovered = self.db.recover_interrupted_pull_tasks()?;
        if recovered > 0 {
            info!("恢复中断的拉流验证任务: {recovered} 个");
            self.notify();
        }
        Ok(recovered)
    }

    pub fn spawn_dispatcher(self: &Arc<Self>) {
        let center = Arc::clone(self);
        tokio::spawn(async move {
            center.dispatch_loop().await;
        });
    }

    async fn dispatch_loop(self: Arc<Self>) {
        loop {
            if let Err(err) = self.try_dispatch().await {
                error!("拉流任务调度失败: {err}");
            }
            self.notify.notified().await;
        }
    }

    async fn try_dispatch(&self) -> Result<()> {
        while self.slots.available_permits() > 0 {
            let Some(task) = self.db.claim_next_pending_pull_task()? else {
                break;
            };
            let permit = self
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| anyhow!("拉流任务并发槽已关闭"))?;
            let center = self.clone();
            tokio::spawn(async move {
                center.run_task(task, permit).await;
                center.notify();
            });
        }
        Ok(())
    }

    pub async fn create_tasks(&self, requests: Vec<CreatePullTaskRequest>) -> Result<Vec<PullTask>> {
        let mut created: Vec<PullTask> = Vec::with_capacity(requests.len());
        for req in requests {
            let protocol = req
                .protocol
                .as_deref()
                .unwrap_or_else(|| detect_protocol(&req.url))
                .to_string();
            if media_server_pull_path(&protocol).is_none() {
                return Err(anyhow!(
                    "不支持的协议: {protocol} (stream_id={})",
                    req.stream_id
                ));
            }

            if let Some(existing) = created.iter().find(|task| {
                task.stream_id == req.stream_id
                    || req
                        .play_item_id
                        .is_some_and(|id| task.play_item_id == Some(id))
            }) {
                created.push(existing.clone());
                continue;
            }

            let task = self.db.create_pull_task(&req, &protocol)?;
            created.push(task);
        }
        self.notify();
        Ok(created)
    }

    pub fn list_tasks(&self, status: Option<&str>, limit: i64) -> Result<Vec<PullTask>> {
        self.db.list_pull_tasks(status, limit)
    }

    pub fn get_task(&self, id: i64) -> Result<Option<PullTask>> {
        self.db.get_pull_task(id)
    }

    pub async fn stop_task(&self, id: i64) -> Result<PullTask> {
        let task = self
            .db
            .get_pull_task(id)?
            .ok_or_else(|| anyhow!("任务不存在: {id}"))?;

        let stop_flag = {
            let mut flags = self.stop_flags.lock().await;
            flags
                .entry(id)
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone()
        };
        stop_flag.store(true, Ordering::SeqCst);

        match task.status.as_str() {
            "pending" => {
                self.db
                    .update_pull_task_status(id, "stopped", Some("用户停止"))?;
            }
            "pulling" | "snapshotting" => {
                let _ = delete_stream(&self.client, &self.media_server_url, &task.stream_id).await;
                self.db
                    .update_pull_task_status(id, "stopped", Some("用户停止"))?;
            }
            _ => {
                return Err(anyhow!("任务状态 {} 不可停止", task.status));
            }
        }

        self.cleanup_stop_flag(id).await;
        self.notify();
        self.db
            .get_pull_task(id)?
            .ok_or_else(|| anyhow!("任务不存在: {id}"))
    }

    pub async fn retry_task(&self, id: i64) -> Result<PullTask> {
        if !self.db.reset_pull_task_for_retry(id)? {
            return Err(anyhow!("仅 failed/stopped/completed 状态的任务可重试"));
        }
        self.notify();
        self.db
            .get_pull_task(id)?
            .ok_or_else(|| anyhow!("任务不存在: {id}"))
    }

    async fn run_task(&self, task: PullTask, _permit: OwnedSemaphorePermit) {
        let task_id = task.id;
        let stop_flag = {
            let mut flags = self.stop_flags.lock().await;
            flags
                .entry(task_id)
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone()
        };
        let should_stop = || stop_flag.load(Ordering::SeqCst);

        let result = self.execute_task(&task, should_stop).await;
        self.cleanup_stop_flag(task_id).await;

        if let Err(err) = result {
            if should_stop() {
                let _ = self
                    .db
                    .update_pull_task_status(task_id, "stopped", Some("用户停止"));
            } else {
                warn!("拉流任务 {} 失败: {err}", task_id);
                let _ = delete_stream(&self.client, &self.media_server_url, &task.stream_id).await;
                let _ = self
                    .db
                    .update_pull_task_status(task_id, "failed", Some(&err.to_string()));
            }
        }
    }

    async fn execute_task(
        &self,
        task: &PullTask,
        should_stop: impl Fn() -> bool + Copy,
    ) -> Result<()> {
        if should_stop() {
            return Err(anyhow!("任务已停止"));
        }

        let (status, body) = submit_pull(
            &self.client,
            &self.media_server_url,
            &task.protocol,
            &task.url,
            &task.stream_id,
        )
        .await
        .map_err(|e| anyhow!(e))?;
        if !status.is_success() {
            let err = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("拉流失败");
            return Err(anyhow!(err.to_string()));
        }

        if should_stop() {
            return Err(anyhow!("任务已停止"));
        }

        let ready = wait_for_stream_keyframe(
            &self.client,
            &self.media_server_url,
            &task.stream_id,
            KEYFRAME_WAIT,
            POLL_INTERVAL,
            should_stop,
        )
        .await
        .map_err(|e| anyhow!(e))?;
        if !ready {
            return Err(anyhow!("等待关键帧超时"));
        }

        if should_stop() {
            return Err(anyhow!("任务已停止"));
        }

        self.db
            .update_pull_task_status(task.id, "snapshotting", None)?;

        let (status, body) = submit_snapshot(&self.client, &self.media_server_url, &task.stream_id)
            .await
            .map_err(|e| anyhow!(e))?;
        if !status.is_success() {
            let err = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("提交截图失败");
            return Err(anyhow!(err.to_string()));
        }

        let snapshot_id = body
            .pointer("/snapshot/id")
            .or_else(|| body.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("截图返回缺少 id"))?
            .to_string();
        self.db
            .update_pull_task_snapshot(task.id, &snapshot_id, "pending")?;

        let snapshot = wait_for_snapshot(
            &self.client,
            &self.media_server_url,
            &snapshot_id,
            SNAPSHOT_WAIT,
            POLL_INTERVAL,
            should_stop,
        )
        .await
        .map_err(|e| anyhow!(e))?;

        let snapshot_status = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed");
        self.db
            .update_pull_task_snapshot(task.id, &snapshot_id, snapshot_status)?;

        let _ = delete_stream(&self.client, &self.media_server_url, &task.stream_id).await;
        self.db
            .update_pull_task_status(task.id, "completed", None)?;
        info!(
            "拉流验证任务 {} 完成 stream='{}' snapshot='{}'",
            task.id, task.stream_id, snapshot_id
        );
        Ok(())
    }

    async fn cleanup_stop_flag(&self, task_id: i64) {
        let mut flags = self.stop_flags.lock().await;
        flags.remove(&task_id);
    }

    pub fn running_count(&self) -> Result<i64> {
        self.db
            .count_pull_tasks_by_status(&["pulling", "snapshotting"])
    }
}
