use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use kithara::platform::{sync::Arc, tokio::task::spawn_blocking};

use crate::{
    native::routes::signal::{encode_signal_payload, encoded_signal_cache_key},
    signal_pcm::SignalLength,
    signal_spec::{SignalFormat, SignalKind, parse_signal_request},
    test_server_state::TestServerState,
    token_store::{TokenRequest, TokenResponse, TokenRoute},
};

pub(crate) fn router() -> Router<Arc<TestServerState>> {
    Router::new().route("/token", post(create_token))
}

async fn create_token(
    State(state): State<Arc<TestServerState>>,
    Json(request): Json<TokenRequest>,
) -> impl IntoResponse {
    match request.route {
        TokenRoute::Signal => register_signal_token(&state, &request).await,
        TokenRoute::Hls => register_hls_token(&state, request),
    }
}

async fn register_signal_token(
    state: &Arc<TestServerState>,
    request: &TokenRequest,
) -> axum::response::Response {
    let Some(raw_kind) = request.signal_kind.as_deref() else {
        return (StatusCode::BAD_REQUEST, "missing `signal_kind`").into_response();
    };
    let Some(spec_with_ext) = request.signal_spec_with_ext.as_deref() else {
        return (StatusCode::BAD_REQUEST, "missing `signal_spec_with_ext`").into_response();
    };

    let signal_request = match SignalKind::try_from(raw_kind)
        .and_then(|kind| parse_signal_request(kind, spec_with_ext))
    {
        Ok(signal_request) => signal_request,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let streams_infinite_wav = matches!(signal_request.format, SignalFormat::Wav)
        && matches!(signal_request.spec.length, SignalLength::Infinite);
    let encoded = if streams_infinite_wav {
        None
    } else {
        let encode_request = signal_request.clone();
        match spawn_blocking(move || encode_signal_payload(&encode_request)).await {
            Ok(Some(encoded)) => Some(encoded),
            Ok(None) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "signal encoding failed")
                    .into_response();
            }
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("signal encoding task failed: {error}"),
                )
                    .into_response();
            }
        }
    };
    let kind = signal_request.spec.kind;
    let path_ext = signal_request.format.path_ext();
    let token = state.insert_signal(signal_request);
    if let Some(encoded) = encoded {
        let token_with_ext = format!("{token}.{path_ext}");
        state.insert_encoded_signal(encoded_signal_cache_key(kind, &token_with_ext), encoded);
    }
    Json(TokenResponse { token }).into_response()
}

fn register_hls_token(
    state: &Arc<TestServerState>,
    request: TokenRequest,
) -> axum::response::Response {
    let Some(spec) = request.hls_spec else {
        return (StatusCode::BAD_REQUEST, "missing `hls_spec`").into_response();
    };

    match state.insert_hls_spec(spec) {
        Ok(token) => Json(TokenResponse { token }).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test(tokio)]
    async fn signal_token_is_published_only_after_its_payload_is_prepared() {
        let state = TestServerState::new();
        let spec = URL_SAFE_NO_PAD.encode(r#"{"frames":1024,"sample_rate":44100,"channels":2}"#);
        let request = TokenRequest {
            route: TokenRoute::Signal,
            signal_kind: Some("sawtooth".to_owned()),
            signal_spec_with_ext: Some(format!("{spec}.wav")),
            hls_spec: None,
        };

        let response = register_signal_token(&state, &request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read token response");
        let response: TokenResponse = serde_json::from_slice(&body).expect("parse token response");
        let token_with_ext = format!("{}.wav", response.token);
        let cache_key = encoded_signal_cache_key(SignalKind::Sawtooth, &token_with_ext);

        assert!(state.get_encoded_signal(&cache_key).is_some());
    }
}
