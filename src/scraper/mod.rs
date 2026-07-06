pub mod m3u_source;

use crate::models::RawPlayItem;

/// 爬虫 trait：各源站爬虫需要实现此接口
#[async_trait::async_trait]
pub trait Scraper: Send + Sync {
    /// 源站名称
    fn source_name(&self) -> &str;
    /// 爬取所有播放地址
    async fn scrape(&self) -> anyhow::Result<Vec<RawPlayItem>>;
}
