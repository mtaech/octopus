//! octopus 薄 bin：装配组合根并启动 axum 服务（#20 ①）。

use std::sync::Arc;

use octopus_api::ai::{build_ai_provider, build_embedding_backend};
use octopus_api::config::load_config_from_disk;
use octopus_api::{router, AppState};
use octopus_engine::{AssetStore, SqliteStore};

#[tokio::main]
async fn main() {
    octopus_api::logging::init();

    let db_path = std::env::var("OCTOPUS_DB").unwrap_or_else(|_| "octopus.db".to_string());
    let addr = std::env::var("OCTOPUS_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

    // 资产库默认与数据库同级；可用 OCTOPUS_ASSETS 覆盖
    let assets_dir = std::env::var("OCTOPUS_ASSETS").unwrap_or_else(|_| "assets".to_string());

    let store = Arc::new(SqliteStore::open(&db_path).await.expect("打开单库失败"));
    let assets = Arc::new(AssetStore::open(&assets_dir).await.expect("打开资产库失败"));
    let cfg = load_config_from_disk();
    let ai = build_ai_provider(&cfg);
    let embedding = build_embedding_backend(&cfg);
    let app = router(AppState::new(store, ai, embedding, assets));

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("绑定端口失败");
    tracing::info!(%addr, db = %db_path, assets = %assets_dir, "octopus api 已启动");
    axum::serve(listener, app).await.expect("服务退出");
}
