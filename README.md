# vcp-iptv-manager

IPTV 播放地址爬取、验证和管理系统，使用 Rust + Axum 实现。单一二进制部署，零外部依赖。

## 功能

- **通用 M3U 播源拉取**: 支持从任意 M3U/M3U8 播放列表 URL 拉取频道和流地址，兼容主播放列表（`#EXT-X-STREAM-INF`）和简单播放列表（`#EXTINF`）
- **播源动态管理**: 通过 REST API 动态添加、删除、启用/禁用播源，无需重启服务
- **流地址验证**: 并发验证 M3U8 流地址可用性（HTTP 状态码 + 内容格式检查），自动提取分辨率（`RESOLUTION`）和码率（`BANDWIDTH`）
- **定时任务调度**: 基于 cron 表达式的定时拉取和验证，启动时自动执行首轮
- **REST API**: 完整的 HTTP API，支持分页查询、多条件筛选、M3U8 导出
- **SQLite 存储**: 内置 SQLite，无需安装数据库服务

## 快速开始

### 环境要求

- Rust 1.75+

### 构建

```bash
cargo build --release
```

### 运行

```bash
# 方式一：使用配置文件（推荐）
cp config.example.toml config.toml   # 首次使用需复制模板
# 编辑 config.toml 中的播源和端口等配置
cargo run --release

# 方式二：纯默认配置（无初始播源）
cargo run --release

# 方式三：环境变量覆盖配置文件中的部分字段
PORT=5001 SCRAPE_INTERVAL_SECS=7200 cargo run --release
```

### 配置文件

配置优先级：**配置文件 `config.toml` → 环境变量覆盖**

首次使用请复制模板：
```bash
cp config.example.toml config.toml
```

配置模板 `config.example.toml`：

```toml
# 数据库路径
db_path = "data/iptv.db"

# HTTP 监听地址和端口
host = "0.0.0.0"
port = 5001

# 播源拉取间隔（秒）
scrape_interval_secs = 3600

# 流地址验证超时（秒）
verify_timeout_secs = 10

# HTTP 请求超时（秒）
request_timeout_secs = 30

# 验证并发数
verify_concurrency = 20

# 初始播源列表（启动时自动注入数据库，已存在的跳过）
[[sources]]
name = "zbds-iptv4"
url = "https://live.zbds.top/tv/iptv4.m3u"
category = "综合"
```

### 环境变量覆盖

| 环境变量 | 对应配置项 | 示例 |
|---------|-----------|------|
| `CONFIG_PATH` | 配置文件路径 | `my-config.toml` |
| `PORT` | 端口 | `5001` |
| `DB_PATH` | 数据库路径 | `/data/iptv.db` |
| `SCRAPE_INTERVAL_SECS` | 拉取间隔 | `7200` |
| `VERIFY_TIMEOUT_SECS` | 验证超时 | `15` |
| `REQUEST_TIMEOUT_SECS` | 请求超时 | `60` |
| `VERIFY_CONCURRENCY` | 验证并发数 | `30` |
| `INITIAL_SOURCES` | 旧格式播源（兼容） | `name,url,cat;...` |

> **提示**: 可在 GitHub 或社区搜索公开的 IPTV M3U 播放列表，将 URL 作为播源添加。

## API 接口

### 接口列表

| 方法 | 路径 | 说明 | 参数 |
|------|------|------|------|
| GET | `/api/stats` | 统计信息 | - |
| GET | `/api/playitems` | 分页查询播放地址 | `channel`, `source`, `is_valid`, `keyword`, `page_num`, `page_size` |
| GET | `/api/playitems/sources` | 获取所有来源列表 | - |
| GET | `/api/playitems/m3u8` | 导出有效地址为 M3U8 | - |
| GET | `/api/channels` | 分页查询频道 | `keyword`, `source`, `page_num`, `page_size` |
| GET | `/api/channels/sources` | 获取频道来源 | - |
| POST | `/api/scrape` | 手动触发完整拉取 | - |
| GET | `/api/sources` | 查询所有播源 | - |
| POST | `/api/sources` | 添加播源 | JSON body |
| DELETE | `/api/sources/{id}` | 删除播源 | - |
| POST | `/api/sources/{id}/toggle` | 切换启/禁用 | `{"enabled": true/false}` |
| POST | `/api/sources/{id}/fetch` | 立即拉取此播源 | - |

### 响应格式

所有接口统一返回：

```json
{
    "code": 0,
    "message": "ok",
    "data": { ... }
}
```

分页查询示例 (`GET /api/playitems?is_valid=true&page_num=1&page_size=20`)：

```json
{
    "code": 0,
    "message": "ok",
    "data": {
        "total": 516,
        "page_num": 1,
        "page_size": 20,
        "items": [
            {
                "id": 1,
                "channel_name": "CCTV-1 综合",
                "url": "http://example.com/cctv1.m3u8",
                "source": "zbds-iptv4",
                "category": "央视",
                "is_valid": true,
                "fail_count": 0,
                "last_checked": "2025-07-06T12:00:00",
                "resolution": "1920x1080",
                "bitrate": 4000000
            }
        ]
    }
}
```

统计信息示例 (`GET /api/stats`)：

```json
{
    "code": 0,
    "message": "ok",
    "data": {
        "total_channels": 0,
        "total_play_items": 523,
        "valid_play_items": 516,
        "invalid_play_items": 7,
        "total_sources": 1,
        "active_sources": 1,
        "sources": [
            {"name": "zbds-iptv4", "total": 523, "valid": 516}
        ]
    }
}
```

## 播源管理

播源支持运行时动态管理，无需重启服务。

```bash
# 添加播源
curl -X POST http://localhost:5000/api/sources \
  -H "Content-Type: application/json" \
  -d '{"name":"我的播源","url":"https://example.com/tv.m3u","category":"综合"}'

# 查询所有播源
curl http://localhost:5000/api/sources

# 启用 / 禁用播源
curl -X POST http://localhost:5000/api/sources/1/toggle \
  -H "Content-Type: application/json" \
  -d '{"enabled":true}'

# 删除播源
curl -X DELETE http://localhost:5000/api/sources/1

# 立即拉取
curl -X POST http://localhost:5000/api/sources/1/fetch

# 导出有效地址为 M3U8 播放列表
curl http://localhost:5000/api/playitems/m3u8
```

## 工作流程

```
                      ┌──────────────────┐
    启动时注入 ──────→│ playlist_sources  │←───── API 动态管理
  (config.toml 播源)   │   (播源注册表)     │     (添加/删除/启禁)
                      └────────┬─────────┘
                               │ 定时 cron 拉取
                               ▼
                 ┌─────────────────────────┐
                 │   M3U 播放列表拉取器       │
                 │  (通用 M3U/M3U8 解析引擎)  │
                 └────────────┬────────────┘
                              │ RawPlayItem[]
                              ▼
                 ┌─────────────────────────┐
                 │      play_items 表        │
                 │   (流地址 + 验证状态)      │
                 └────────────┬────────────┘
                              │ 定时验证 (20并发)
                              ▼
                 ┌─────────────────────────┐
                 │      M3U8 流验证器        │
                 │  (HTTP状态 + 内容格式检查) │
                 └────────────┬────────────┘
                              │
                              ▼
                 ┌─────────────────────────┐
                 │    REST API 查询 / 导出   │
                 │   GET /api/playitems     │
                 │   GET /api/playitems/m3u8│
                 └─────────────────────────┘
```

## 项目结构

```
├── Cargo.toml            # 项目依赖 (Axum, reqwest, rusqlite, tokio...)
├── config.example.toml   # 配置文件模板（复制为 config.toml 使用）
├── README.md
└── src/
    ├── main.rs           # 入口：初始化 → 调度器 → HTTP 服务
    ├── lib.rs            # AppState 定义（共享 DB / Verifier / HTTP Client）
    ├── config.rs         # 环境变量配置管理
    ├── db.rs             # SQLite 数据库操作（频道 / 播放地址 / 播源 / 统计）
    ├── models.rs         # 数据模型（Channel, PlayItem, PlaylistSource, Stats）
    ├── verifier.rs       # 流地址并发验证器（HTTP 200 + M3U8 格式检查）
    ├── scheduler.rs      # 基于 tokio-cron-scheduler 的定时任务
    ├── api/
    │   ├── mod.rs        # 路由注册
    │   ├── channels.rs   # /api/channels 频道查询 API
    │   ├── playitems.rs  # /api/playitems 播放地址查询/导出 API
    │   └── sources.rs    # /api/sources 播源管理 API
    └── scraper/
        ├── mod.rs        # Scraper trait 定义
        └── m3u_source.rs # 通用 M3U/M3U8 拉取 + 解析引擎（含测试用例）
```

## 技术栈

- **Web 框架**: [Axum 0.8](https://github.com/tokio-rs/axum)
- **HTTP 客户端**: [reqwest 0.12](https://github.com/seanmonstar/reqwest)
- **数据库**: [rusqlite 0.34](https://github.com/rusqlite/rusqlite) (SQLite, 内置编译)
- **调度器**: [tokio-cron-scheduler 0.13](https://github.com/mvniekerk/tokio-cron-scheduler)
- **异步运行时**: [Tokio](https://tokio.rs)
- **日志**: [tracing](https://github.com/tokio-rs/tracing)
- **序列化**: [serde](https://serde.rs) / serde_json / toml
- **HTML 解析**: [scraper](https://crates.io/crates/scraper)
- **URL 解析**: [url](https://crates.io/crates/url)
