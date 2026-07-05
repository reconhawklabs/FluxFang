#[tokio::test]
async fn migrations_apply_and_postgis_present() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL set for tests");
    let pool = fluxfang_db::connect(&url).await.unwrap();
    fluxfang_db::run_migrations(&pool).await.unwrap();
    let (ext,): (bool,) = sqlx::query_as(
        "select exists(select 1 from pg_extension where extname='postgis')")
        .fetch_one(&pool).await.unwrap();
    assert!(ext);
}
