use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use kithara::platform::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::test_server_state::TestServerState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NetworkRequest {
    pub online: bool,
}

pub(crate) fn router() -> Router<Arc<TestServerState>> {
    Router::new().route("/control/network", post(set_network))
}

async fn set_network(
    State(state): State<Arc<TestServerState>>,
    Json(request): Json<NetworkRequest>,
) -> impl IntoResponse {
    state.set_network_online(request.online);
    StatusCode::NO_CONTENT
}

/// Reject every data route while the global network switch is offline.
///
/// `/control/*` remains reachable so callers can restore the network, and
/// `/health` remains available for process-level liveness checks.
pub(crate) async fn network_guard(
    State(state): State<Arc<TestServerState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let exempt = path.starts_with("/control/") || path == "/health";
    if !exempt && !state.network_online() {
        return (StatusCode::SERVICE_UNAVAILABLE, "network offline").into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{kithara, test_server_state::TestServerState};

    #[kithara::test]
    fn network_starts_online() {
        let state = TestServerState::new();
        assert!(state.network_online(), "server must start reachable");
    }

    #[kithara::test]
    fn network_switch_flips_both_ways() {
        let state = TestServerState::new();
        state.set_network_online(false);
        assert!(
            !state.network_online(),
            "switch must take the server offline"
        );
        state.set_network_online(true);
        assert!(state.network_online(), "switch must bring the server back");
    }
}
