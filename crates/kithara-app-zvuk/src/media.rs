use serde::{Deserialize, Deserializer};
use url::Url;

use crate::TrackId;

/// A track identity paired with its stream-resolution result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaTrack {
    /// Stable identity supplied by the catalogue.
    pub(crate) id: TrackId,
    #[serde(deserialize_with = "Option::deserialize")]
    /// Resolved stream, or no available stream.
    pub(crate) stream_v3: Option<HlsStream>,
}

/// The optional HLS URL and server-supplied expiration value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct HlsStream {
    #[serde(deserialize_with = "optional_hls")]
    /// A nonempty HTTP HLS URL, or no playable URL.
    pub(crate) hls: Option<Url>,
    #[serde(deserialize_with = "Option::deserialize")]
    /// Expiration text supplied by the server, without a non-expiry guarantee.
    pub(crate) expire: Option<String>,
}

fn optional_hls<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Url>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => Url::parse(&value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}
