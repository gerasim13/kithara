#![cfg(not(target_arch = "wasm32"))]

use kithara_integration_tests::TestServerHelper;

/// Лейн собирается и достаёт тест-сервер. Единственный зелёный тест в
/// `suite_laba` — он существует, чтобы отличить «лейн сломан» от
/// «ловушка красная».
#[kithara::test(tokio)]
async fn lane_reaches_test_server() {
    let helper = TestServerHelper::new().await;
    assert!(
        helper.base_url().as_str().starts_with("http://127.0.0.1:"),
        "test server must bind loopback, got {}",
        helper.base_url()
    );
}
