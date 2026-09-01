use std::collections::BTreeMap;

use kithara::{hls::SizeProbeMethod, net::Compression};
use serde::{Deserialize, Serialize};

/// Everything one configuration document can say. Sections default to empty, so
/// a document names only what it changes.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Document {
    pub(crate) assets: Assets,
    pub(crate) drm: Drm,
    pub(crate) network: Network,
    pub(crate) playback: Playback,
    pub(crate) playlist: Playlist,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Playback {
    pub(crate) crossfade_seconds: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Network {
    pub(crate) compression: Vec<CompressionAlgorithm>,
    pub(crate) should_accept_invalid_certs: Option<bool>,
    pub(crate) size_probe_method: SizeProbeMethod,
}

impl Network {
    /// Fold the declared algorithms into the flags the HTTP client offers. An
    /// empty list is `Compression::empty()` -- negotiation off.
    #[must_use]
    pub(crate) fn compression(&self) -> Compression {
        self.compression
            .iter()
            .fold(Compression::empty(), |flags, &algorithm| {
                flags.union(algorithm.into())
            })
    }
}

/// One `Accept-Encoding` algorithm. Named rather than spelled as bit flags,
/// because a document reads as a list of names.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CompressionAlgorithm {
    Gzip,
    Deflate,
    #[serde(alias = "br")]
    Brotli,
    Zstd,
}

impl From<CompressionAlgorithm> for Compression {
    fn from(algorithm: CompressionAlgorithm) -> Self {
        match algorithm {
            CompressionAlgorithm::Gzip => Self::GZIP,
            CompressionAlgorithm::Deflate => Self::DEFLATE,
            CompressionAlgorithm::Brotli => Self::BROTLI,
            CompressionAlgorithm::Zstd => Self::ZSTD,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Assets {
    pub(crate) cache_identity: Vec<CacheIdentityRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CacheIdentityRule {
    pub(crate) domains: Vec<String>,
    pub(crate) query_parameters: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Playlist {
    pub(crate) tracks: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Drm {
    pub(crate) providers: Vec<DrmProvider>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DrmProvider {
    pub(crate) name: String,
    pub(crate) domains: Vec<String>,
    /// Cipher key for this provider. A document references a secret through
    /// `$KITHARA_...`; expansion has already run by the time this parses.
    pub(crate) cipher_key: String,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) seed: SeedSpec,
}

/// Shape of the per-request `X-Encrypted-Key` salt.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SeedSpec {
    pub(crate) alphabet: SeedAlphabet,
    pub(crate) length: usize,
}

impl Default for SeedSpec {
    fn default() -> Self {
        Self {
            alphabet: SeedAlphabet::Hex,
            length: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SeedAlphabet {
    #[default]
    Hex,
    Alphanumeric,
}

#[cfg(test)]
mod tests {
    use kithara::{hls::SizeProbeMethod, net::Compression};

    use super::Document;
    use crate::baked::BAKED_DOCUMENT;

    #[kithara::test(native, flash(false))]
    fn the_baked_document_parses_under_the_schema() {
        let document: Document =
            serde_yaml_ng::from_str(BAKED_DOCUMENT).expect("the baked document matches the schema");

        assert!(
            !document.playlist.tracks.is_empty(),
            "the baked document ships a playlist"
        );
        assert!(
            !document.drm.providers.is_empty(),
            "the baked document ships DRM providers"
        );
        assert!(
            !document.assets.cache_identity.is_empty(),
            "the baked document ships cache-identity rules"
        );
    }

    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_refused_and_named() {
        let error = serde_yaml_ng::from_str::<Document>("playback:\n  crossfade_second: 5.0\n")
            .expect_err("a typo must not pass silently");

        assert!(error.to_string().contains("crossfade_second"), "{error}");
    }

    #[kithara::test(native, flash(false))]
    fn an_empty_document_is_all_defaults() {
        let document: Document = serde_yaml_ng::from_str("{}").expect("an empty document is valid");

        assert_eq!(document.network.size_probe_method, SizeProbeMethod::Head);
        assert!(document.playlist.tracks.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn compression_algorithms_map_to_their_flags() {
        let document: Document =
            serde_yaml_ng::from_str("network:\n  compression: [gzip, deflate]\n")
                .expect("valid document");

        assert_eq!(
            document.network.compression(),
            Compression::GZIP.union(Compression::DEFLATE)
        );
    }

    #[kithara::test(native, flash(false))]
    fn an_empty_compression_list_disables_negotiation() {
        let document: Document =
            serde_yaml_ng::from_str("network:\n  compression: []\n").expect("valid document");

        assert_eq!(document.network.compression(), Compression::empty());
    }
}
