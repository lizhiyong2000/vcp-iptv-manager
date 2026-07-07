use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tracing::info;

use crate::models::{Channel, PlayItem, PlaylistSource, RawPlayItem, SourceStats, Stats};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn new(db_path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        db.migrate_channels()?;
        info!("数据库初始化完成: {}", db_path);
        Ok(db)
    }

    fn init_tables(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS channels (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT    NOT NULL,
                source      TEXT    NOT NULL,
                category    TEXT,
                logo_url    TEXT,
                created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, source)
            );

            CREATE TABLE IF NOT EXISTS play_items (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name  TEXT    NOT NULL,
                url           TEXT    NOT NULL,
                source        TEXT    NOT NULL,
                category      TEXT,
                is_valid      INTEGER DEFAULT 0,
                fail_count    INTEGER DEFAULT 0,
                last_checked  TIMESTAMP,
                resolution    TEXT,
                bitrate       INTEGER,
                created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(url)
            );

            CREATE TABLE IF NOT EXISTS playlist_sources (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                name          TEXT    NOT NULL,
                url           TEXT    NOT NULL UNIQUE,
                category      TEXT,
                enabled       INTEGER DEFAULT 1,
                last_count    INTEGER,
                last_status   TEXT,
                last_fetch_at TIMESTAMP,
                created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_play_items_channel ON play_items(channel_name);
            CREATE INDEX IF NOT EXISTS idx_play_items_source  ON play_items(source);
            CREATE INDEX IF NOT EXISTS idx_play_items_valid   ON play_items(is_valid);
            CREATE INDEX IF NOT EXISTS idx_channels_source    ON channels(source);
            ",
        )?;
        Ok(())
    }

    /// 迁移：从 play_items 同步频道到 channels 表（仅当 channels 为空时）
    fn migrate_channels(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))?;
        if channel_count > 0 {
            return Ok(());
        }
        let affected = conn.execute(
            "INSERT OR IGNORE INTO channels (name, source, category)
             SELECT DISTINCT channel_name, source, category FROM play_items",
            [],
        )?;
        if affected > 0 {
            info!("数据迁移: 从 play_items 同步 {} 个频道到 channels 表", affected);
        }
        Ok(())
    }

    // ---- 频道操作 ----

    pub fn upsert_channel(
        &self,
        name: &str,
        source: &str,
        category: Option<&str>,
        logo_url: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channels (name, source, category, logo_url)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name, source) DO UPDATE SET
                category = COALESCE(?3, category),
                logo_url = COALESCE(?4, logo_url),
                updated_at = CURRENT_TIMESTAMP",
            params![name, source, category, logo_url],
        )?;
        Ok(())
    }

    pub fn count_channels(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn list_channels(
        &self,
        keyword: Option<&str>,
        source: Option<&str>,
        page_num: i32,
        page_size: i32,
    ) -> Result<(Vec<Channel>, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut conditions = vec!["1=1".to_string()];
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(kw) = keyword {
            conditions.push(format!("name LIKE ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(format!("%{}%", kw)));
        }
        if let Some(src) = source {
            conditions.push(format!("source = ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(src.to_string()));
        }

        let where_clause = conditions.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) FROM channels WHERE {}", where_clause);
        let total: i64 = {
            let mut stmt = conn.prepare(&count_sql)?;
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                bind_values.iter().map(|v| v.as_ref()).collect();
            stmt.query_row(params_ref.as_slice(), |r| r.get(0))?
        };

        let offset = ((page_num - 1) * page_size).max(0);
        let query_sql = format!(
            "SELECT id, name, source, category, logo_url, created_at, updated_at
             FROM channels WHERE {} ORDER BY id LIMIT ?{} OFFSET ?{}",
            where_clause,
            bind_values.len() + 1,
            bind_values.len() + 2,
        );
        bind_values.push(Box::new(page_size));
        bind_values.push(Box::new(offset));

        let mut stmt = conn.prepare(&query_sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|v| v.as_ref()).collect();
        let items = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok(Channel {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    source: row.get(2)?,
                    category: row.get(3)?,
                    logo_url: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((items, total))
    }

    /// 根据 ID 查询频道
    pub fn get_channel(&self, id: i64) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, source, category, logo_url, created_at, updated_at
             FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Channel {
                id: row.get(0)?,
                name: row.get(1)?,
                source: row.get(2)?,
                category: row.get(3)?,
                logo_url: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// 查询某频道的播放地址列表（按 channel_name + source 匹配）
    pub fn get_channel_playitems(
        &self,
        channel_name: &str,
        source: &str,
        page_num: i32,
        page_size: i32,
    ) -> Result<(Vec<PlayItem>, i64)> {
        let conn = self.conn.lock().unwrap();

        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM play_items WHERE channel_name = ?1 AND source = ?2",
            params![channel_name, source],
            |r| r.get(0),
        )?;

        let offset = ((page_num - 1) * page_size).max(0);
        let mut stmt = conn.prepare(
            "SELECT id, channel_name, url, source, category, is_valid, fail_count,
                    last_checked, resolution, bitrate, created_at, updated_at
             FROM play_items
             WHERE channel_name = ?1 AND source = ?2
             ORDER BY is_valid DESC, id DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let items = stmt
            .query_map(params![channel_name, source, page_size, offset], |row| {
                Ok(PlayItem {
                    id: row.get(0)?,
                    channel_name: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    category: row.get(4)?,
                    is_valid: row.get(5)?,
                    fail_count: row.get(6)?,
                    last_checked: row.get(7)?,
                    resolution: row.get(8)?,
                    bitrate: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((items, total))
    }

    // ---- 播放地址操作 ----

    pub fn upsert_play_item(&self, item: &RawPlayItem) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "INSERT INTO play_items (channel_name, url, source, category, resolution)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(url) DO UPDATE SET
                channel_name = ?1,
                source      = ?3,
                category    = COALESCE(?4, category),
                resolution  = COALESCE(?5, resolution),
                updated_at  = CURRENT_TIMESTAMP",
            params![
                item.channel_name,
                item.url,
                item.source,
                item.category,
                item.resolution,
            ],
        )?;
        Ok(affected > 0)
    }

    /// 批量插入播放地址
    /// 已存在的条目会重置验证状态，以便重新验证
    pub fn upsert_play_items(&self, items: &[RawPlayItem]) -> Result<usize> {
        let mut count = 0;
        let conn = self.conn.lock().unwrap();
        conn.execute("BEGIN", [])?;
        for item in items {
            let affected = conn.execute(
                "INSERT INTO play_items (channel_name, url, source, category, resolution)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(url) DO UPDATE SET
                    channel_name = ?1,
                    source      = ?3,
                    category    = COALESCE(?4, category),
                    resolution  = COALESCE(?5, resolution),
                    is_valid    = 0,
                    fail_count  = 0,
                    last_checked = NULL,
                    updated_at  = CURRENT_TIMESTAMP",
                params![
                    item.channel_name,
                    item.url,
                    item.source,
                    item.category,
                    item.resolution,
                ],
            )?;
            if affected > 0 {
                count += 1;
            }
            // 同步写入 channels 表（按 name+source 去重）
            let _ = conn.execute(
                "INSERT INTO channels (name, source, category)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(name, source) DO UPDATE SET
                    category   = COALESCE(?3, category),
                    updated_at = CURRENT_TIMESTAMP",
                params![item.channel_name, item.source, item.category],
            );
        }
        conn.execute("COMMIT", [])?;
        Ok(count)
    }

    /// 更新验证状态
    pub fn update_play_item_validity(
        &self,
        id: i64,
        is_valid: bool,
        resolution: Option<&str>,
        bitrate: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE play_items SET
                is_valid     = ?1,
                fail_count   = CASE WHEN ?1 = 1 THEN 0 ELSE fail_count + 1 END,
                last_checked = CURRENT_TIMESTAMP,
                resolution   = COALESCE(?2, resolution),
                bitrate      = COALESCE(?3, bitrate),
                updated_at   = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![is_valid, resolution, bitrate, id],
        )?;
        Ok(())
    }

    /// 清理播源中已失效的播放地址（URL 不在最新拉取列表中）
    pub fn cleanup_stale_items(&self, source: &str, fresh_urls: &[String]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        if fresh_urls.is_empty() {
            return Ok(0);
        }
        // 构建 NOT IN 占位符（从 ?2 开始，?1 已用于 source）
        let placeholders: Vec<String> = fresh_urls
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "DELETE FROM play_items WHERE source = ?1 AND url NOT IN ({})",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params.push(Box::new(source.to_string()));
        for url in fresh_urls {
            params.push(Box::new(url.clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|v| v.as_ref()).collect();
        let deleted = stmt.execute(params_ref.as_slice())?;
        Ok(deleted)
    }

    /// 获取全部需要验证的播放地址
    pub fn get_unverified_items(&self) -> Result<Vec<PlayItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_name, url, source, category, is_valid, fail_count,
                    last_checked, resolution, bitrate, created_at, updated_at
             FROM play_items
             WHERE last_checked IS NULL
                OR last_checked < datetime('now', '-1 hours')
             ORDER BY last_checked IS NULL DESC, last_checked ASC",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(PlayItem {
                    id: row.get(0)?,
                    channel_name: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    category: row.get(4)?,
                    is_valid: row.get(5)?,
                    fail_count: row.get(6)?,
                    last_checked: row.get(7)?,
                    resolution: row.get(8)?,
                    bitrate: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn count_play_items(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM play_items", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn list_play_items(
        &self,
        channel: Option<&str>,
        source: Option<&str>,
        is_valid: Option<bool>,
        keyword: Option<&str>,
        page_num: i32,
        page_size: i32,
    ) -> Result<(Vec<PlayItem>, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut conditions = vec!["1=1".to_string()];
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ch) = channel {
            conditions.push(format!("channel_name LIKE ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(format!("%{}%", ch)));
        }
        if let Some(src) = source {
            conditions.push(format!("source = ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(src.to_string()));
        }
        if let Some(valid) = is_valid {
            conditions.push(format!("is_valid = ?{}", bind_values.len() + 1));
            bind_values.push(Box::new(valid));
        }
        if let Some(kw) = keyword {
            conditions.push(format!(
                "(channel_name LIKE ?{n} OR url LIKE ?{n})",
                n = bind_values.len() + 1
            ));
            bind_values.push(Box::new(format!("%{}%", kw)));
        }

        let where_clause = conditions.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) FROM play_items WHERE {}", where_clause);
        let total: i64 = {
            let mut stmt = conn.prepare(&count_sql)?;
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                bind_values.iter().map(|v| v.as_ref()).collect();
            stmt.query_row(params_ref.as_slice(), |r| r.get(0))?
        };

        let offset = ((page_num - 1) * page_size).max(0);
        let query_sql = format!(
            "SELECT id, channel_name, url, source, category, is_valid, fail_count,
                    last_checked, resolution, bitrate, created_at, updated_at
             FROM play_items WHERE {}
             ORDER BY is_valid DESC, id DESC
             LIMIT ?{} OFFSET ?{}",
            where_clause,
            bind_values.len() + 1,
            bind_values.len() + 2,
        );
        bind_values.push(Box::new(page_size));
        bind_values.push(Box::new(offset));

        let mut stmt = conn.prepare(&query_sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|v| v.as_ref()).collect();
        let items = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok(PlayItem {
                    id: row.get(0)?,
                    channel_name: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    category: row.get(4)?,
                    is_valid: row.get(5)?,
                    fail_count: row.get(6)?,
                    last_checked: row.get(7)?,
                    resolution: row.get(8)?,
                    bitrate: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((items, total))
    }

    /// 获取所有有效的播放地址（用于导出 M3U8）
    pub fn get_valid_play_items(&self) -> Result<Vec<PlayItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_name, url, source, category, is_valid, fail_count,
                    last_checked, resolution, bitrate, created_at, updated_at
             FROM play_items WHERE is_valid = 1
             ORDER BY channel_name",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(PlayItem {
                    id: row.get(0)?,
                    channel_name: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    category: row.get(4)?,
                    is_valid: row.get(5)?,
                    fail_count: row.get(6)?,
                    last_checked: row.get(7)?,
                    resolution: row.get(8)?,
                    bitrate: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn get_sources(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT source FROM play_items ORDER BY source")?;
        let sources = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(sources)
    }

    // ---- 播源管理 ----

    /// 添加一个播源 URL
    pub fn add_playlist_source(&self, name: &str, url: &str, category: Option<&str>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO playlist_sources (name, url, category) VALUES (?1, ?2, ?3)",
            params![name, url, category],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 删除播源
    pub fn delete_playlist_source(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM playlist_sources WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// 切换播源启用状态
    pub fn toggle_playlist_source(&self, id: i64, enabled: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE playlist_sources SET enabled = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![enabled, id],
        )?;
        Ok(n > 0)
    }

    /// 列出所有播源
    pub fn list_playlist_sources(&self) -> Result<Vec<PlaylistSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, category, enabled, last_count, last_status,
                    last_fetch_at, created_at, updated_at
             FROM playlist_sources ORDER BY id",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(PlaylistSource {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    url: row.get(2)?,
                    category: row.get(3)?,
                    enabled: row.get(4)?,
                    last_count: row.get(5)?,
                    last_status: row.get(6)?,
                    last_fetch_at: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    /// 获取所有启用的播源
    pub fn get_enabled_playlist_sources(&self) -> Result<Vec<PlaylistSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, category, enabled, last_count, last_status,
                    last_fetch_at, created_at, updated_at
             FROM playlist_sources WHERE enabled = 1 ORDER BY id",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(PlaylistSource {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    url: row.get(2)?,
                    category: row.get(3)?,
                    enabled: row.get(4)?,
                    last_count: row.get(5)?,
                    last_status: row.get(6)?,
                    last_fetch_at: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    /// 更新播源拉取状态
    pub fn update_playlist_source_status(
        &self,
        id: i64,
        count: i32,
        status: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE playlist_sources SET
                last_count = ?1, last_status = ?2, last_fetch_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?3",
            params![count, status, id],
        )?;
        Ok(())
    }

    /// 启动时：将配置中的初始播源插入数据库（已存在的忽略）
    pub fn ensure_playlist_sources(&self, sources: &[(String, String, Option<String>)]) -> Result<usize> {
        let mut count = 0;
        for (name, url, category) in sources {
            let conn = self.conn.lock().unwrap();
            let affected = conn.execute(
                "INSERT OR IGNORE INTO playlist_sources (name, url, category) VALUES (?1, ?2, ?3)",
                params![name, url, category],
            )?;
            drop(conn);
            if affected > 0 {
                count += 1;
            }
        }
        Ok(count)
    }

    // ---- 统计 ----

    pub fn get_stats(&self) -> Result<Stats> {
        let conn = self.conn.lock().unwrap();
        let total_channels: i64 =
            conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))?;
        let total_play_items: i64 =
            conn.query_row("SELECT COUNT(*) FROM play_items", [], |r| r.get(0))?;
        let valid_play_items: i64 = conn.query_row(
            "SELECT COUNT(*) FROM play_items WHERE is_valid = 1",
            [],
            |r| r.get(0),
        )?;
        let invalid_play_items = total_play_items - valid_play_items;
        let total_sources: i64 =
            conn.query_row("SELECT COUNT(*) FROM playlist_sources", [], |r| r.get(0))?;
        let active_sources: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playlist_sources WHERE enabled = 1",
            [],
            |r| r.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT source,
                    COUNT(*) as total,
                    SUM(CASE WHEN is_valid = 1 THEN 1 ELSE 0 END) as valid
             FROM play_items GROUP BY source ORDER BY total DESC",
        )?;
        let source_stats = stmt
            .query_map([], |row| {
                Ok(SourceStats {
                    name: row.get(0)?,
                    total: row.get(1)?,
                    valid: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Stats {
            total_channels,
            total_play_items,
            valid_play_items,
            invalid_play_items,
            total_sources,
            active_sources,
            sources: source_stats,
        })
    }
}
