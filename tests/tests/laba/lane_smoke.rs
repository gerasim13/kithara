#![cfg(not(target_arch = "wasm32"))]

use kithara_integration_tests::TestServerHelper;

/// Proves the lane builds and reaches the test server. This is the only
/// green test in `suite_laba`, distinguishing a broken lane from a red trap.
#[kithara::test(tokio)]
async fn lane_reaches_test_server() {
    let helper = TestServerHelper::new().await;
    assert!(
        helper.base_url().as_str().starts_with("http://127.0.0.1:"),
        "test server must bind loopback, got {}",
        helper.base_url()
    );
}

/// The network switch is reachable over HTTP and cannot lock itself out:
/// `/control/*` must keep responding while data routes are offline.
#[kithara::test(tokio)]
async fn network_switch_is_reachable_over_http() {
    let helper = TestServerHelper::new().await;
    let control = helper.url("/control/network");
    let health = helper.url("/health");
    let client = reqwest::Client::new();

    let offline = client
        .post(control.clone())
        .json(&serde_json::json!({ "online": false }))
        .send()
        .await
        .expect("control endpoint must answer");
    assert_eq!(offline.status(), 204);

    let blocked = client
        .get(helper.url("/behavior/missing"))
        .send()
        .await
        .expect("server must still accept connections while offline");
    assert_eq!(
        blocked.status(),
        503,
        "data routes must be dead while the switch is off"
    );

    let alive = client
        .get(health)
        .send()
        .await
        .expect("health must survive");
    assert_eq!(alive.status(), 200);

    let online = client
        .post(control)
        .json(&serde_json::json!({ "online": true }))
        .send()
        .await
        .expect("control endpoint must stay reachable while offline");
    assert_eq!(online.status(), 204);
}
