#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use futures::FutureExt;
use kithara::platform::time::Duration;
use kithara_integration_tests::{TestServerHelper, kithara};

use super::sync_product_matrix::{Provider, sources};

/// Every sync provider materialises two sources or reports why it is blocked.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn every_provider_materialises_two_sources() {
    let server = TestServerHelper::new().await;
    let mut blocked = Vec::new();
    for provider in Provider::ALL {
        let paths = match std::panic::AssertUnwindSafe(sources(*provider, 2, &server))
            .catch_unwind()
            .await
        {
            Ok(paths) => paths,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    message.starts_with("BLOCKED_FIXTURE"),
                    "{provider:?}: {message}"
                );
                blocked.push(format!("{provider:?}: {message}"));
                continue;
            }
        };
        assert_eq!(paths.len(), 2, "{provider:?}");
        for path in &paths {
            let exists = path.starts_with("http") || Path::new(path).is_file();
            assert!(
                exists,
                "{provider:?}: {path} is neither a served URL nor a file"
            );
        }
    }
    if std::env::var_os("KITHARA_REMOTE_FIXTURES").is_some_and(|value| !value.is_empty()) {
        assert!(
            blocked.is_empty(),
            "requested remote fixtures are unavailable: {blocked:?}"
        );
    }
    eprintln!("blocked providers: {blocked:?}");
}
