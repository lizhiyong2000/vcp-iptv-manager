use anyhow::Result;
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::db::Database;
use crate::models::PlayItem;

/// M3U8 地址验证器
pub struct Verifier {
    db: Arc<Database>,
    config: Config,
    client: reqwest::Client,
}

impl Verifier {
    pub fn new(db: Arc<Database>, config: Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.verify_timeout_secs))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .expect("Failed to create HTTP client");

        Self { db, config, client }
    }

    /// 验证所有待验证的播放地址
    pub async fn verify_unchecked(&self) -> Result<VerificationResult> {
        info!("开始验证播放地址可用性...");
        let items = self.db.get_unverified_items()?;

        if items.is_empty() {
            info!("没有需要验证的播放地址");
            return Ok(VerificationResult {
                total: 0,
                valid: 0,
                invalid: 0,
            });
        }

        let total = items.len();
        let valid = self
            .verify_batch(items, self.config.verify_concurrency)
            .await;

        let result = VerificationResult {
            total: total as i32,
            valid,
            invalid: total as i32 - valid,
        };
        info!(
            "验证完成: 总数={}, 有效={}, 无效={}",
            result.total, result.valid, result.invalid,
        );
        Ok(result)
    }

    /// 并发验证一批播放地址
    async fn verify_batch(&self, items: Vec<PlayItem>, concurrency: usize) -> i32 {
        let valid_count = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let limited = stream::iter(items)
            .map(|item| {
                let client = self.client.clone();
                let db = self.db.clone();
                let valid_count = valid_count.clone();
                async move {
                    match Self::verify_single(&client, &item).await {
                        Ok((is_valid, resolution, bitrate)) => {
                            if is_valid {
                                valid_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            if let Err(e) = db.update_play_item_validity(
                                item.id,
                                is_valid,
                                resolution.as_deref(),
                                bitrate,
                            ) {
                                error!("更新验证状态失败 id={}: {}", item.id, e);
                            }
                            debug!(
                                "验证 [{}] {}: {}",
                                if is_valid { "有效" } else { "无效" },
                                item.channel_name,
                                item.url
                            );
                        }
                        Err(e) => {
                            // 网络错误也标记为无效
                            if let Err(e2) =
                                db.update_play_item_validity(item.id, false, None, None)
                            {
                                error!("更新验证状态失败 id={}: {}", item.id, e2);
                            }
                            debug!("验证失败 [{}] {}: {:?}", item.channel_name, item.url, e);
                        }
                    }
                }
            })
            .buffer_unordered(concurrency);

        limited.collect::<Vec<_>>().await;
        valid_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 验证单个 M3U8 地址
    async fn verify_single(
        client: &reqwest::Client,
        item: &PlayItem,
    ) -> Result<(bool, Option<String>, Option<i64>)> {
        let resp = client.get(&item.url).send().await?;

        // 检查 HTTP 状态码
        if !resp.status().is_success() {
            return Ok((false, None, None));
        }

        // 获取响应体前几 KB 检查是否为有效 M3U8
        let content_length = resp.content_length();
        let bytes = resp.bytes().await?;
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);

        // 检查是否包含 M3U8 特征
        let is_m3u8 = text.contains("#EXTM3U")
            || text.contains("#EXTINF")
            || text.contains("#EXT-X-STREAM-INF");

        if !is_m3u8 {
            return Ok((false, None, None));
        }

        // 提取分辨率（如果有）
        let mut resolution = None;
        let mut bitrate = content_length.map(|l| l as i64);

        for line in text.lines() {
            if line.starts_with("#EXT-X-STREAM-INF") {
                if let Some(res_str) = line
                    .split("RESOLUTION=")
                    .nth(1)
                    .and_then(|s| s.split(',').next())
                {
                    resolution = Some(res_str.to_string());
                }
                if bitrate.is_none() {
                    if let Some(br_str) = line
                        .split("BANDWIDTH=")
                        .nth(1)
                        .and_then(|s| s.split(',').next())
                    {
                        bitrate = br_str.parse::<i64>().ok();
                    }
                }
            }
        }

        Ok((true, resolution, bitrate))
    }
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub total: i32,
    pub valid: i32,
    pub invalid: i32,
}
