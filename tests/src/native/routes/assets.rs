use std::path::PathBuf;

use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use kithara::platform::sync::Arc;
use kithara_test_fixtures::assets::by_name;
use tower_http::services::ServeDir;

use crate::test_server_state::TestServerState;

pub(crate) fn router() -> Router<Arc<TestServerState>> {
    Router::new()
        .nest_service("/assets", ServeDir::new(assets_dir()))
        .route("/streamhq", get(streamhq))
}

pub(crate) fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root from tests/")
        .join("assets")
}

/// Mirror the production `cdn-edge.zvq.me/track/streamhq?id=*` URL shape:
/// the path carries no file extension, so the only way a client can guess
/// the codec is via the `Content-Type` header on the response. Used by
/// `file_replay_from_warm_cache_mp3_no_extension` to pin that cold-cache
/// reload preserves the mime hint.
///
/// `name` is `{accessor}.{ext}`, the same spelling the `/signal` route takes,
/// so both routes serve one generated body under one name.
async fn streamhq(Query(params): Query<StreamHqQuery>) -> Response {
    let Some((name, _ext)) = params.name.rsplit_once('.') else {
        return (
            StatusCode::BAD_REQUEST,
            format!("`{}` carries no file extension", params.name),
        )
            .into_response();
    };
    let Some(asset) = by_name(name) else {
        return (
            StatusCode::NOT_FOUND,
            format!("no generated asset is named `{name}`"),
        )
            .into_response();
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        asset.entry().content_type.parse().expect("mime"),
    );
    headers.insert(header::ACCEPT_RANGES, "bytes".parse().expect("static"));
    (headers, Body::from(asset.bytes())).into_response()
}

#[derive(serde::Deserialize)]
struct StreamHqQuery {
    name: String,
}
