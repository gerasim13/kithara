use std::collections::BTreeMap;

#[cfg(feature = "broadcast")]
use kithara::broadcast::BroadcastSettings;
use kithara::{
    analysis::BeatAnalysisSettingsPatch,
    assets::FlushSettings,
    hls::SizeProbeMethod,
    host::HostSettings,
    net::NetSettings,
    stream::dl::DownloaderSettings,
    worker::{ComputePoolSettings, WorkerSettings},
};
use serde::Deserialize;

use crate::{config::AppSettings, pools::PoolsSection};

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
    pub(crate) beat: BeatAnalysisSettingsPatch,
    #[cfg(feature = "broadcast")]
    pub(crate) broadcast: BroadcastSettings,
    pub(crate) downloader: DownloaderSettings,
    pub(crate) drm: Drm,
    pub(crate) flush: FlushSettings,
    pub(crate) host: HostSettings,
    pub(crate) net: NetSettings,
    pub(crate) network: Network,
    pub(crate) playback: Playback,
    pub(crate) playlist: Playlist,
    pub(crate) pools: PoolsSection,
    pub(crate) worker: WorkerSettings,
    pub(crate) worker_pool: Option<ComputePoolSettings>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Playback {
    pub(crate) crossfade_seconds: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Network {
    pub(crate) size_probe_method: SizeProbeMethod,
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
    use std::num::NonZeroUsize;

    use kithara::hls::SizeProbeMethod;

    use super::{ComputePoolSettings, Document};
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
        assert!(
            document.worker_pool.is_none(),
            "a document naming no compute pool leaves the crate default standing"
        );
        assert!(
            document.worker.max_compute_tasks.is_none(),
            "a document naming no worker section leaves the crate default standing"
        );
        assert!(
            document.pools.budget_bytes.is_none(),
            "a document naming no pools section leaves the region budget standing"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_pools_section_names_one_pool_without_touching_the_other() {
        let document: Document = serde_yaml_ng::from_str("pools:\n  bytes:\n    max_buffers: 64\n")
            .expect("a valid document");

        assert_eq!(document.pools.bytes.max_buffers, Some(64));
        assert!(
            document.pools.samples.max_buffers.is_none(),
            "naming one pool leaves the other empty"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_worker_section_names_the_compute_task_ceiling() {
        let document: Document =
            serde_yaml_ng::from_str("worker:\n  max_compute_tasks: 4\n").expect("a valid document");

        assert_eq!(
            document.worker.max_compute_tasks.map(NonZeroUsize::get),
            Some(4)
        );
        assert!(
            document.worker_pool.is_none(),
            "naming the worker section does not name a pool"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_worker_pool_document_names_the_owned_mode() {
        let document: Document = serde_yaml_ng::from_str(
            "worker_pool:\n  mode: owned\n  name: analysis\n  threads: 2\n",
        )
        .expect("a valid compute-pool document parses");

        match document.worker_pool {
            Some(ComputePoolSettings::Owned { name, threads }) => {
                assert_eq!(name, "analysis");
                assert_eq!(threads.get(), 2);
            }
            other => panic!("expected an owned compute pool, got {other:?}"),
        }
    }

    #[kithara::test(native, flash(false))]
    fn a_network_compression_key_is_rejected() {
        let error = serde_yaml_ng::from_str::<Document>("network:\n  compression: []\n")
            .expect_err("compression moved to the net section");

        assert!(error.to_string().contains("compression"), "{error}");
    }
}
