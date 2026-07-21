mod common;

use fluxfang_db::models::NewDataSource;
use fluxfang_db::DataSourceRepo;

/// Grab a currently-free localhost port by binding to :0 and releasing it.
async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

#[tokio::test]
async fn listener_binds_serves_health_then_stops() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let src = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".to_string(),
            mode: "listener".to_string(),
            interface: None,
            config: serde_json::json!({
                "bind_ip": "127.0.0.1", "bind_port": port, "enrollment_window_secs": 900
            }),
        },
    )
    .await
    .unwrap();

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(src.id).await;

    // Row is running and the health endpoint answers.
    let row = DataSourceRepo::get(&pool, src.id).await.unwrap().unwrap();
    assert_eq!(row.status, "running");
    let url = format!("http://127.0.0.1:{port}/sensor/health");
    let resp = reqwest::get(&url).await.expect("health request");
    assert_eq!(resp.status().as_u16(), 200);

    mgr.stop(src.id).await;
    let row = DataSourceRepo::get(&pool, src.id).await.unwrap().unwrap();
    assert_eq!(row.status, "stopped");
    // Port no longer accepts connections. Bound with a short timeout since a
    // connection attempt to a just-closed port can occasionally hang rather
    // than fail promptly (see sensor_listener.rs's graceful-shutdown note);
    // both a prompt error and a timeout mean "gone".
    let gone =
        match tokio::time::timeout(std::time::Duration::from_secs(2), reqwest::get(&url)).await {
            Ok(result) => result.is_err(),
            Err(_timeout) => true,
        };
    assert!(gone, "health endpoint should be gone after stop");
}
