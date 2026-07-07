use crate::models::RawPlayItem;
use crate::scraper::Scraper;
use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use url::Url;

/// 通用 M3U/M3U8 播放列表拉取器
///
/// 从播源 URL 拉取 M3U/M3U8 文件并解析频道条目。
/// 支持两种格式：
/// - 简单播放列表 (#EXTINF 条目)
/// - 主播放列表 (#EXT-X-STREAM-INF，包含子流 URL)
pub struct M3uPlaylistFetcher {
    pub source_name: String,
    pub playlist_url: String,
    pub category: Option<String>,
    client: reqwest::Client,
}

impl M3uPlaylistFetcher {
    pub fn new(
        source_name: String,
        playlist_url: String,
        category: Option<String>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            source_name,
            playlist_url,
            category,
            client,
        }
    }
}

#[async_trait::async_trait]
impl Scraper for M3uPlaylistFetcher {
    fn source_name(&self) -> &str {
        &self.source_name
    }

    async fn scrape(&self) -> Result<Vec<RawPlayItem>> {
        info!("[{}] 拉取播源: {}", self.source_name, self.playlist_url);
        let content = self.fetch_content(&self.playlist_url).await?;

        if content.trim().is_empty() {
            warn!("[{}] 播源返回空内容", self.source_name);
            return Ok(Vec::new());
        }

        // 判断是否为 M3U/M3U8 格式
        if !content.contains("#EXTM3U") {
            warn!("[{}] 返回内容不是有效 M3U 格式", self.source_name);
            return Ok(Vec::new());
        }

        // 判断清单类型
        if content.contains("#EXT-X-STREAM-INF") {
            debug!("[{}] 检测到主播放列表，解析子流", self.source_name);
            self.parse_master_playlist(&content).await
        } else {
            debug!("[{}] 检测到简单播放列表", self.source_name);
            Ok(self.parse_simple_playlist(&content))
        }
    }
}

impl M3uPlaylistFetcher {
    /// 下载 M3U/M3U8 文件内容
    async fn fetch_content(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .context("请求播源 URL 失败")?;

        if !resp.status().is_success() {
            anyhow::bail!("播源返回 HTTP {}: {}", resp.status(), url);
        }

        // 读取响应体（限制最大 10MB 防止内存爆炸）
        let bytes = resp.bytes().await.context("读取播源内容失败")?;
        if bytes.len() > 10 * 1024 * 1024 {
            anyhow::bail!("播源内容过大 ({} bytes)，跳过", bytes.len());
        }

        // 尝试 UTF-8 解码，失败则用 lossy 方式处理
        let text = String::from_utf8(bytes.to_vec())
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());

        Ok(text)
    }

    /// 解析简单播放列表（#EXTINF 条目）
    ///
    /// 格式示例:
    /// ```text
    /// #EXTM3U
    /// #EXTINF:-1 tvg-name="CCTV1" tvg-logo="..." group-title="央视",CCTV-1 综合
    /// http://example.com/cctv1.m3u8
    /// #EXTINF:-1,湖南卫视
    /// http://example.com/hunan.m3u8
    /// ```
    fn parse_simple_playlist(&self, content: &str) -> Vec<RawPlayItem> {
        let mut items = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            if line.starts_with("#EXTINF") {
                let channel_name = self.parse_extinf_name(line);
                let category = self.parse_extinf_attr(line, "group-title");
                let logo_url = self.parse_extinf_attr(line, "tvg-logo");
                let resolution = self.parse_extinf_attr(line, "tvg-resolution");

                // 下一行是 URL
                i += 1;
                while i < lines.len() {
                    let next_line = lines[i].trim();
                    if !next_line.is_empty() && !next_line.starts_with('#') {
                        let resolved_url =
                            self.resolve_url(&self.playlist_url, next_line);
                        // 只保留 HLS 流地址（http/https），丢弃 rtp/rtsp/udp 等非 HLS 协议
                        if Self::is_hls_url(&resolved_url) {
                            items.push(RawPlayItem {
                                channel_name: channel_name.clone(),
                                url: resolved_url,
                                source: self.source_name.clone(),
                                category: category.or_else(|| self.category.clone()),
                                resolution: resolution.clone(),
                            });
                        } else {
                            debug!(
                                "[{}] 丢弃非 HLS 地址: {} -> {}",
                                self.source_name, channel_name, resolved_url
                            );
                        }

                        // 同时记录 logo（存到 resolution 字段后面的实际使用有待扩展）
                        // 此处 logo_url 如有需要在后续扩展
                        let _ = logo_url;
                        break;
                    } else if next_line.starts_with('#') && !next_line.starts_with("#EXTINF")
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            i += 1;
        }

        info!(
            "[{}] 解析简单播放列表: {} 个频道",
            self.source_name,
            items.len()
        );
        items
    }

    /// 解析主播放列表（#EXT-X-STREAM-INF）
    ///
    /// 主播放列表包含多个不同码率的子流。
    /// 通常取其第一个子流拉取进一步解析。
    ///
    /// 格式示例:
    /// ```text
    /// #EXTM3U
    /// #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1920x1080
    /// http://example.com/channel_hd.m3u8
    /// #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360
    /// http://example.com/channel_sd.m3u8
    /// ```
    async fn parse_master_playlist(&self, content: &str) -> Result<Vec<RawPlayItem>> {
        // 对主播放列表：尝试递归拉取第一个合适的子流作为简单播放列表
        // 获取子流 URL 列表（质量从低到高排序，优先用最高质量的）
        let sub_urls = self.extract_sub_urls(content);
        if sub_urls.is_empty() {
            warn!("[{}] 主播放列表中未找到子流 URL", self.source_name);
            return Ok(Vec::new());
        }

        // 优先拉取最高分辨率的子流作为代理
        // （已按分辨率排序，最后一个可能是最高的）
        if let Some(best_url) = sub_urls.last() {
            debug!("[{}] 递归拉取子流: {}", self.source_name, best_url);
            match self.fetch_content(best_url).await {
                Ok(sub_content) => {
                    if sub_content.contains("#EXTM3U") {
                        // 子流是简单播放列表
                        if !sub_content.contains("#EXT-X-STREAM-INF") {
                            Ok(self.parse_simple_playlist(&sub_content))
                        } else {
                            // 嵌套的主播放列表，停止递归
                            warn!("[{}] 子流仍为主播放列表，停止递归", self.source_name);
                            Ok(Vec::new())
                        }
                    } else {
                        // 可能是 TS 流等，不是 M3U 格式
                        warn!("[{}] 子流不是有效 M3U 格式", self.source_name);
                        Ok(Vec::new())
                    }
                }
                Err(e) => {
                    warn!("[{}] 拉取子流失败: {}", self.source_name, e);
                    Ok(Vec::new())
                }
            }
        } else {
            Ok(Vec::new())
        }
    }

    /// 提取主播放列表中的子流 URL
    fn extract_sub_urls(&self, content: &str) -> Vec<String> {
        let mut urls = Vec::new();
        let mut resolutions: Vec<(Option<i32>, String)> = Vec::new();

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim();
            if line.starts_with("#EXT-X-STREAM-INF") {
                // 提取分辨率用于排序
                let resolution = line
                    .split("RESOLUTION=")
                    .nth(1)
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| {
                        let parts: Vec<&str> = s.split('x').collect();
                        if parts.len() == 2 {
                            parts[0].parse::<i32>().ok()
                        } else {
                            None
                        }
                    });

                // 下一行是 URL
                i += 1;
                while i < lines.len() {
                    let next = lines[i].trim();
                    if !next.is_empty() && !next.starts_with('#') {
                        let resolved = self.resolve_url(&self.playlist_url, next);
                        resolutions.push((resolution, resolved));
                        break;
                    }
                    i += 1;
                }
            }
            i += 1;
        }

        // 按分辨率排序（从低到高）
        resolutions.sort_by_key(|(res, _)| res.unwrap_or(0));

        for (_, url) in resolutions {
            urls.push(url);
        }

        debug!(
            "[{}] 找到 {} 个子流 URL",
            self.source_name,
            urls.len()
        );
        urls
    }

    /// 从 #EXTINF 行提取频道名称
    ///
    /// 支持格式:
    /// - `#EXTINF:-1,CCTV-1 综合` → "CCTV-1 综合"
    /// - `#EXTINF:-1 tvg-name="CCTV1",CCTV-1 综合` → "CCTV-1 综合"
    /// - `#EXTINF:-1 tvg-name="CCTV1" tvg-logo="...",CCTV-1 综合` → "CCTV-1 综合"
    fn parse_extinf_name(&self, line: &str) -> String {
        // 优先取逗号后面的显示名
        if let Some(comma_pos) = line.rfind(',') {
            let display_name = line[comma_pos + 1..].trim();
            if !display_name.is_empty() {
                return display_name.to_string();
            }
        }

        // 兜底取 tvg-name 属性
        self.parse_extinf_attr(line, "tvg-name")
            .unwrap_or_else(|| "未知频道".to_string())
    }

    /// 从 #EXTINF 行提取指定属性值
    ///
    /// 支持格式: `key="value"` 或 `key=value`
    fn parse_extinf_attr(&self, line: &str, attr: &str) -> Option<String> {
        let prefix = format!("{}=\"", attr);
        if let Some(start) = line.find(&prefix) {
            let start = start + prefix.len();
            if let Some(end) = line[start..].find('"') {
                return Some(line[start..start + end].to_string());
            }
        }

        // 尝试无引号格式
        let prefix_no_quote = format!("{}=", attr);
        if let Some(start) = line.find(&prefix_no_quote) {
            let start = start + prefix_no_quote.len();
            // 无引号时值直到第一个空格或逗号结束
            let rest = &line[start..];
            let end = rest.find(|c: char| c == ' ' || c == ',');
            let value = match end {
                Some(pos) => &rest[..pos],
                None => rest,
            };
            if !value.is_empty() && value != "\"" {
                return Some(value.to_string());
            }
        }

        None
    }

    /// 验证单个流地址是否可访问（HTTP 200 + M3U 格式）
    pub async fn verify_url(client: &reqwest::Client, url: &str) -> (bool, String) {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    return (false, format!("HTTP {}", status.as_u16()));
                }
                // 读取前几 KB 检查是否为有效 M3U/M3U8
                match resp.bytes().await {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(
                            &bytes[..bytes.len().min(4096)],
                        );
                        let is_m3u = text.contains("#EXTM3U")
                            || text.contains("#EXTINF")
                            || text.contains("#EXT-X-STREAM-INF");
                        if is_m3u {
                            (true, "M3U 有效".to_string())
                        } else {
                            (false, "非 M3U 格式".to_string())
                        }
                    }
                    Err(e) => (false, format!("读取失败: {}", e)),
                }
            }
            Err(e) => (false, format!("请求失败: {}", e)),
        }
    }

    /// 判断是否为有效 HLS 播放地址（仅 http/https）
    fn is_hls_url(url: &str) -> bool {
        url.starts_with("http://") || url.starts_with("https://")
    }

    /// 相对 URL 转为绝对 URL
    fn resolve_url(&self, base: &str, relative: &str) -> String {
        let relative = relative.trim();
        // 已经是绝对 URL
        if relative.starts_with("http://") || relative.starts_with("https://") {
            return relative.to_string();
        }

        // 尝试用 url crate 解析
        if let Ok(base_url) = Url::parse(base) {
            if let Ok(resolved) = base_url.join(relative) {
                return resolved.to_string();
            }
        }

        // 兜底：简单拼接
        if relative.starts_with('/') {
            if let Some(domain_end) = base.find("://") {
                let domain_start = domain_end + 3;
                if let Some(path_start) = base[domain_start..].find('/') {
                    let domain = &base[..domain_start + path_start];
                    format!("{}{}", domain, relative)
                } else {
                    format!("{}{}", base, relative)
                }
            } else {
                relative.to_string()
            }
        } else {
            // 相对路径，相对于父目录
            if let Some(last_slash) = base.rfind('/') {
                format!("{}/{}", &base[..last_slash], relative)
            } else {
                format!("{}/{}", base, relative)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::PlayItem;
    use futures::stream::{self, StreamExt};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const TEST_PLAYLIST_URL: &str = "https://live.zbds.top/tv/iptv4.m3u";
    const SOURCE_NAME: &str = "zbds-iptv4";

    /// 创建测试用 HTTP 客户端
    fn make_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .expect("Failed to build HTTP client")
    }

    /// 验证 + 保存到数据库并入库（可被 API 查询）
    ///
    /// 运行: cargo test --lib -- test_seed_zbds_to_db --nocapture
    #[tokio::test]
    async fn test_seed_zbds_to_db() {
        let client = make_client();
        let db = Arc::new(Database::new("data/iptv.db").expect("打开数据库失败"));

        let fetcher = M3uPlaylistFetcher::new(
            SOURCE_NAME.to_string(),
            TEST_PLAYLIST_URL.to_string(),
            None,
            client.clone(),
        );

        // ============ 1. 拉取 ============
        println!("\n========================================");
        println!("正在拉取播源: {}", TEST_PLAYLIST_URL);
        println!("========================================");

        let items = match fetcher.scrape().await {
            Ok(items) => {
                println!("✅ 解析到 {} 个频道", items.len());
                items
            }
            Err(e) => {
                eprintln!("❌ 拉取失败: {}", e);
                return;
            }
        };

        if items.is_empty() {
            println!("⚠️  播源返回 0 个频道");
            return;
        }

        // ============ 2. 全部入库 ============
        println!("\n📥 写入数据库...");
        match db.upsert_play_items(&items) {
            Ok(n) => println!("✅ 新增/更新 {} 条播放地址", n),
            Err(e) => eprintln!("❌ 写入失败: {}", e),
        }

        // ============ 3. 获取待验证的地址并逐个验证 ============
        println!("\n🔍 开始验证流地址可用性... (并发: 20)");

        let unverified = match db.get_unverified_items() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("❌ 查询待验证项失败: {}", e);
                return;
            }
        };

        // 只验证来自 zbds-iptv4 的地址
        let to_verify: Vec<PlayItem> = unverified
            .into_iter()
            .filter(|p| p.source == SOURCE_NAME)
            .collect();

        if to_verify.is_empty() {
            println!("⚠️  没有需要验证的地址（可能已全部验证过）");
            print_db_summary(&db);
            return;
        }

        let total = to_verify.len();
        println!("待验证: {} 条", total);

        let verify_client = make_client();
        let verified_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let valid_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let limited = stream::iter(to_verify.iter())
            .map(|item| {
                let client = verify_client.clone();
                let db = db.clone();
                let verified_count = verified_count.clone();
                let valid_count = valid_count.clone();
                async move {
                    let (is_valid, reason) = M3uPlaylistFetcher::verify_url(&client, &item.url).await;
                    verified_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    // 更新数据库中的验证状态
                    let _ = db.update_play_item_validity(item.id, is_valid, None, None);

                    let n = verified_count.load(std::sync::atomic::Ordering::Relaxed);
                    if is_valid {
                        valid_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if n % 50 == 0 || n == total {
                            println!("  [{}/{}] ✅ {}", n, total, item.channel_name);
                        }
                    } else {
                        if n % 20 == 0 || n == total {
                            println!("  [{}/{}] ❌ {} ({})", n, total, item.channel_name, reason);
                        }
                    }
                }
            })
            .buffer_unordered(20);

        limited.collect::<Vec<_>>().await;

        let v = valid_count.load(std::sync::atomic::Ordering::Relaxed);
        let i = total - v;

        println!("\n========================================");
        println!("📊 验证结果汇总");
        println!("========================================");
        println!("  总数: {}", total);
        println!("  有效: {} ({:.1}%)", v, (v as f64 / total as f64) * 100.0);
        println!("  无效: {} ({:.1}%)", i, (i as f64 / total as f64) * 100.0);

        // ============ 4. 数据库汇总 ============
        print_db_summary(&db);

        assert!(!items.is_empty(), "播源应返回至少一个频道");
    }

    fn print_db_summary(db: &Arc<Database>) {
        match db.get_stats() {
            Ok(stats) => {
                println!("\n📊 数据库当前状态:");
                println!("  频道数: {}", stats.total_channels);
                println!("  播放地址总数: {}", stats.total_play_items);
                println!("  有效地址: {}", stats.valid_play_items);
                println!("  无效地址: {}", stats.invalid_play_items);
                println!("  播源数: {} (激活: {})", stats.total_sources, stats.active_sources);
                if !stats.sources.is_empty() {
                    println!("\n  各播源统计:");
                    for s in &stats.sources {
                        println!(
                            "    {} - 总数: {}, 有效: {}",
                            s.name, s.total, s.valid
                        );
                    }
                }
            }
            Err(e) => eprintln!("查询统计失败: {}", e),
        }
    }

    #[tokio::test]
    async fn test_fetch_and_verify_zbds_iptv4() {
        let client = make_client();
        let fetcher = M3uPlaylistFetcher::new(
            SOURCE_NAME.to_string(),
            TEST_PLAYLIST_URL.to_string(),
            None,
            client.clone(),
        );

        // ============ 1. 拉取并解析 ============
        println!("\n========================================");
        println!("正在拉取播源: {}", TEST_PLAYLIST_URL);
        println!("========================================");

        let items = match fetcher.scrape().await {
            Ok(items) => items,
            Err(e) => {
                eprintln!("❌ 拉取失败: {}", e);
                return;
            }
        };

        if items.is_empty() {
            println!("⚠️  播源返回 0 个频道");
            return;
        }

        // ============ 2. 统计概览 ============
        println!("\n📊 播源概览:");
        println!("  - 总频道数: {}", items.len());

        // 按分类统计
        let mut categories: BTreeMap<String, usize> = BTreeMap::new();
        let mut has_resolution = 0;
        for item in &items {
            let cat = item.category.as_deref().unwrap_or("(无分类)");
            *categories.entry(cat.to_string()).or_default() += 1;
            if item.resolution.is_some() {
                has_resolution += 1;
            }
        }
        println!("  - 含分辨率信息: {}/{}", has_resolution, items.len());
        println!("\n  分类统计:");
        for (cat, count) in &categories {
            println!("    {} : {} 个频道", cat, count);
        }

        // 展示前 20 个频道
        println!("\n📋 频道列表 (前 20 个):");
        for (i, item) in items.iter().take(20).enumerate() {
            let res = item.resolution.as_deref().unwrap_or("-");
            let cat = item.category.as_deref().unwrap_or("-");
            println!(
                "  {:3}. {:20} | 分类: {:10} | 分辨率: {:8} | URL: {}",
                i + 1,
                item.channel_name,
                cat,
                res,
                item.url
            );
        }
        if items.len() > 20 {
            println!("  ... (共 {} 个频道，仅展示前 20 个)", items.len());
        }

        // ============ 3. 验证流地址 ============
        println!("\n========================================");
        println!("🔍 开始验证流地址可用性... (并发数: 20)");
        println!("========================================");

        let verify_client = make_client();
        let total = items.len();
        let verified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let valid_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let invalid_urls = Arc::new(std::sync::Mutex::new(Vec::new()));

        let limited = stream::iter(items.iter())
            .map(|item| {
                let client = verify_client.clone();
                let verified = verified.clone();
                let valid_count = valid_count.clone();
                let invalid_urls = invalid_urls.clone();
                async move {
                    let (is_valid, reason) = M3uPlaylistFetcher::verify_url(&client, &item.url).await;
                    verified.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if is_valid {
                        valid_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        println!(
                            "  ✅ [{}/{}] {} -> {}",
                            verified.load(std::sync::atomic::Ordering::Relaxed),
                            total,
                            item.channel_name,
                            item.url
                        );
                    } else {
                        println!(
                            "  ❌ [{}/{}] {} -> {} ({})",
                            verified.load(std::sync::atomic::Ordering::Relaxed),
                            total,
                            item.channel_name,
                            item.url,
                            reason
                        );
                        invalid_urls.lock().unwrap().push((
                            item.channel_name.clone(),
                            item.url.clone(),
                            reason,
                        ));
                    }
                }
            })
            .buffer_unordered(20);

        limited.collect::<Vec<_>>().await;

        // ============ 4. 结果汇总 ============
        let v = valid_count.load(std::sync::atomic::Ordering::Relaxed);
        let i = total - v;
        println!("\n========================================");
        println!("📊 验证结果汇总");
        println!("========================================");
        println!("  总数: {}", total);
        println!("  有效: {} ({:.1}%)", v, (v as f64 / total as f64) * 100.0);
        println!("  无效: {} ({:.1}%)", i, (i as f64 / total as f64) * 100.0);

        let bad = invalid_urls.lock().unwrap();
        if !bad.is_empty() {
            println!("\n❌ 无效流地址详情:");
            for (name, url, reason) in bad.iter() {
                println!("  - {} | {} | {}", name, url, reason);
            }
        }

        assert!(!items.is_empty(), "播源应返回至少一个频道");
    }
}
