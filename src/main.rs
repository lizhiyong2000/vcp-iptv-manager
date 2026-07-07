use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use vcp_iptv_manager::api;
use vcp_iptv_manager::config::Config;
use vcp_iptv_manager::db::Database;
use vcp_iptv_manager::scheduler;
use vcp_iptv_manager::verifier::Verifier;
use vcp_iptv_manager::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vcp_iptv_manager=info".into()),
        )
        .init();

    info!("vcp-iptv-manager 启动中...");

    // 加载配置
    let config = Config::from_file_or_default();
    info!("配置: {:?}", config);

    // 初始化数据库
    let db = Arc::new(Database::new(&config.db_path)?);

    // 注入初始播源（来自配置，已存在的自动忽略）
    let initial_sources = config.parse_initial_sources();
    if !initial_sources.is_empty() {
        match db.ensure_playlist_sources(&initial_sources) {
            Ok(n) => info!("注入初始播源: {} 个新增", n),
            Err(e) => tracing::warn!("注入初始播源失败: {}", e),
        }
    }

    // 创建 HTTP 客户端（共享连接池）
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()?;

    // 初始化验证器
    let verifier = Arc::new(Verifier::new(db.clone(), config.clone()));

    // 构建应用状态
    let state = Arc::new(AppState {
        db: db.clone(),
        verifier: verifier.clone(),
        client: client.clone(),
        media_manager_url: config.media_manager_url.clone(),
    });

    // 启动定时任务调度器（M3U 播源拉取 + 验证）
    let _scheduler_handle =
        scheduler::start_scheduler(db.clone(), verifier.clone(), config.clone(), client.clone())
            .await?;

    // CORS 配置
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 构建 API 路由
    let app = api::build_router().with_state(state).layer(cors);

    // 启动 HTTP 服务
    let addr = format!("{}:{}", config.host, config.port);
    info!("HTTP 服务监听: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
