use fluxfang_api::AppState;

#[tokio::main]
async fn main() {
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set (see .env.example)");

    let pool = fluxfang_db::connect(&database_url)
        .await
        .expect("connect to Postgres");
    fluxfang_db::run_migrations(&pool)
        .await
        .expect("run database migrations");

    let state = AppState::new(pool);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, fluxfang_api::app(state))
        .await
        .unwrap();
}
