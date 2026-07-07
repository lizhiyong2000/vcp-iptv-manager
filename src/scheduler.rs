use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::scraper::m3u_source::M3uPlaylistFetcher;
use crate::scraper::Scraper;
use crate::verifier::Verifier;

/// 启动定时任务调度器
pub async fn start_scheduler(
    db: Arc<Database>,
    verifier: Arc<Verifier>,
    config: Config,
    client: reqwest::Client,
) -> anyhow::Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;

    // 定时拉取任务：每隔 scrape_interval_secs 执行一次
    // cron 格式: sec min hour dom month dow (6 字段)
    let hours = config.scrape_interval_secs / 3600;
    let minute = (config.scrape_interval_secs % 3600) / 60;
    let cron_expr = if hours >= 2 {
        format!("0 {} */{} * * *", minute, hours)
    } else if hours == 1 {
        format!("0 {} * * * *", minute) // 每小时一次
    } else {
        format!("0 */{} * * * *", config.scrape_interval_secs / 60)
    };
    info!("播源拉取任务 cron: {}", cron_expr);

    let db_for_scrape = db.clone();
    let client_for_scrape = client.clone();
    let scrape_job = Job::new_async(cron_expr.as_str(), {
        move |_uuid, _lock| {
            let db = db_for_scrape.clone();
            let client = client_for_scrape.clone();
            Box::pin(async move {
                run_scrape_cycle(db, client).await;
            })
        }
    })?;
    scheduler.add(scrape_job).await?;

    // 定时验证任务：每小时执行一次
    let verifier_for_job = verifier.clone();
    let verify_job = Job::new_async("0 0 * * * *", {
        move |_uuid, _lock| {
            let verifier = verifier_for_job.clone();
            Box::pin(async move {
                match verifier.verify_unchecked().await {
                    Ok(result) => info!(
                        "定时验证完成: 总={}, 有效={}, 无效={}",
                        result.total, result.valid, result.invalid
                    ),
                    Err(e) => error!("定时验证失败: {}", e),
                }
            })
        }
    })?;
    scheduler.add(verify_job).await?;

    // 启动时立即执行一次拉取+验证
    {
        let db_clone = db.clone();
        let verifier_clone = verifier.clone();
        let client_clone = client.clone();

        tokio::spawn(async move {
            info!("首次启动，立即执行播源拉取任务...");
            run_scrape_cycle(db_clone.clone(), client_clone).await;

            info!("首次拉取完成，开始验证...");
            match verifier_clone.verify_unchecked().await {
                Ok(result) => info!(
                    "首次验证完成: 总={}, 有效={}, 无效={}",
                    result.total, result.valid, result.invalid
                ),
                Err(e) => error!("首次验证失败: {}", e),
            }
        });
    }

    scheduler.start().await?;
    info!(
        "定时任务调度器已启动，拉取间隔: {}s (cron: {})，验证间隔: 每小时",
        config.scrape_interval_secs, cron_expr
    );

    Ok(scheduler)
}

/// 执行一次完整爬取周期：拉取所有启用的 M3U 播源
async fn run_scrape_cycle(db: Arc<Database>, client: reqwest::Client) {
    info!("定时播源拉取任务开始...");

    // 拉取所有启用的 M3U 播源
    match db.get_enabled_playlist_sources() {
        Ok(sources) => {
            if sources.is_empty() {
                info!("没有启用的 M3U 播源");
            } else {
                info!("拉取 {} 个 M3U 播源...", sources.len());
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
                                // 收集本次拉取的所有 URL
                                let fresh_urls: Vec<String> = items.iter().map(|i| i.url.clone()).collect();
                                match db.upsert_play_items(&items) {
                                    Ok(n) => {
                                        info!(
                                            "播源 [{}]: 解析 {} 个频道, 新增/更新 {} 个",
                                            source_name, count, n
                                        );
                                        // 清理已不在播源中的过期条目
                                        match db.cleanup_stale_items(&source_name, &fresh_urls) {
                                            Ok(d) if d > 0 => {
                                                info!("播源 [{}]: 清理 {} 条过期播放地址", source_name, d);
                                            }
                                            Err(e) => warn!("播源 [{}]: 清理过期条目失败: {}", source_name, e),
                                            _ => {}
                                        }
                                    }
                                    Err(e) => error!("播源 [{}]: 保存失败: {}", source_name, e),
                                }
                            }
                            let _ = db.update_playlist_source_status(source.id, count, "ok");
                        }
                        Err(e) => {
                            warn!("播源 [{}]: 拉取失败: {}", source_name, e);
                            let _ = db.update_playlist_source_status(source.id, 0, "error");
                        }
                    }
                    // 播源间适当间隔，避免被限制
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        Err(e) => error!("查询播源列表失败: {}", e),
    }

    info!("定时播源拉取任务完成");
}
