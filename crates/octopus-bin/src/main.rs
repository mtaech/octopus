//! octopus 薄 bin：装配组合根并启动 axum 服务（#20 ①）。

use std::sync::Arc;

use octopus_ai::ScriptedProvider;
use octopus_api::{router, AppState};
use octopus_engine::SqliteStore;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn".into()),
        )
        .init();

    let db_path = std::env::var("OCTOPUS_DB").unwrap_or_else(|_| "octopus.db".to_string());
    let addr = std::env::var("OCTOPUS_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

    let store = Arc::new(SqliteStore::open(&db_path).await.expect("打开单库失败"));
    let ai = Arc::new(ScriptedProvider);
    let app = router(AppState::new(store, ai));

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("绑定端口失败");
    tracing::info!(%addr, db = %db_path, "octopus api 已启动");
    axum::serve(listener, app).await.expect("服务退出");
}
