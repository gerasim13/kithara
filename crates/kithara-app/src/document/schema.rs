use std::collections::BTreeMap;

use kithara::{
    hls::SizeProbeMethod,
    net::{Compression, NetSettings},
};
use serde::Deserialize;

use crate::config::AppSettings;

/// Everything one configuration document can say. Sections default to empty, so
/// a document names only what it changes.
///
/// Deserialize-only on purpose: by the time a document is typed its references
/// are resolved, so this tree holds cipher keys and header secrets in the clear.
/// Rendering the configuration is [`Config::dump`]'s job, and it prints the
/// pre-expansion source instead.
///
/// [`Config::dump`]: crate::document::Config::dump
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Document {
    pub(crate) app: AppSettings,
    pub(crate) assets: Assets,
    pub(crate) drm: Drm,
    pub(crate) net: NetSettings,
    pub(crate) network: Network,
    pub(crate) playback: Playback,
    pub(crate) playlist: Playlist,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Playback {
    pub(crate) crossfade_seconds: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Network {
    /// Read as the list of algorithm names `kithara-net` names its flags by.
    pub(crate) compression: Compression,
    pub(crate) should_accept_invalid_certs: Option<bool>,
    pub(crate) size_probe_method: SizeProbeMethod,
}

impl Default for Network {
    /// A document that names no coding offers none. `Compression` has no
    /// `Default` of its own because the option's built default is every
    /// coding, and that is not what a silent document asks for.
    fn default() -> Self {
        Self {
            compression: Compression::empty(),
            should_accept_invalid_certs: None,
            size_probe_method: SizeProbeMethod::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Assets {
    pub(crate) cache_identity: Vec<CacheIdentityRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CacheIdentityRule {
    pub(crate) domains: Vec<String>,
    pub(crate) query_parameters: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Playlist {
    pub(crate) tracks: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Drm {
    pub(crate) providers: Vec<DrmProvider>,
}

#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
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
    fn an_empty_compression_list_disables_negotiation() {
        let document: Document =
            serde_yaml_ng::from_str("network:\n  compression: []\n").expect("valid document");

        assert_eq!(document.network.compression, Compression::empty());
    }
}
