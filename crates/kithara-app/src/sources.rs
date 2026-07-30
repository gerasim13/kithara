use std::fmt;

use kithara::{
    hls::KeyOptions,
    play::Headers,
    prelude::{PlaybackResamplerBackend, ResourceConfig},
};
use kithara_queue::TrackSource;
use url::Url;

use crate::config::AppConfig;

/// Build a [`TrackSource`] with the app's shared network, asset, and DRM state.
/// Matching policy headers also apply to playlist and segment requests.
#[must_use]
pub fn build_source(url: &str, config: &AppConfig) -> TrackSource {
    build_resource_config(url, config).map_or_else(
        || TrackSource::Uri(url.to_string()),
        |cfg| TrackSource::Config(Box::new(cfg)),
    )
}

/// Build a [`ResourceConfig`] for `url` with the app's shared stores,
/// downloader, flush hub, and DRM routing injected (same as
/// [`build_source`]). `None` when `url` is not a valid source.
#[must_use]
pub(crate) fn build_resource_config(
    url: &str,
    config: &AppConfig,
) -> Option<ResourceConfig<PlaybackResamplerBackend>> {
    let builder = match ResourceConfig::for_src(url) {
        Ok(builder) => builder,
        Err(e) => {
            tracing::error!(%e, "failed to build ResourceConfig");
            return None;
        }
    };
    let registry = config.drm.registry();
    let keys = if registry.is_empty() {
        KeyOptions::default()
    } else {
        KeyOptions::builder().key_registry(registry.clone()).build()
    };
    let headers = Url::parse(url).ok().and_then(|parsed| {
        let host = parsed.host_str().unwrap_or("");
        config.drm.resource_headers(&parsed).map_or_else(
            || {
                tracing::debug!(
                    host,
                    "drm: no registry rule for host -- plain (non-DRM) resource path"
                );
                None
            },
            |h| {
                tracing::info!(
                    host,
                    header_names = ?HeaderNames(&h),
                    "drm: registry rule matched, forwarding headers to resource"
                );
                Some(h)
            },
        )
    });
    Some(
        builder
            .downloader(config.downloader.clone())
            .byte_pool(config.byte_pool.clone())
            .pcm_pool(config.pcm_pool.clone())
            .store(config.store.clone())
            .keys(keys)
            .maybe_headers(headers)
            .size_probe_method(config.size_probe_method)
            .build(),
    )
}

/// Build the full set of [`TrackSource`]s from `config.tracks`.
#[must_use]
pub fn build_sources(config: &AppConfig) -> Vec<TrackSource> {
    config
        .tracks
        .iter()
        .map(|url| build_source(url, config))
        .collect()
}

struct HeaderNames<'a>(&'a Headers);

impl fmt::Debug for HeaderNames<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, _)| name))
            .finish()
    }
}
