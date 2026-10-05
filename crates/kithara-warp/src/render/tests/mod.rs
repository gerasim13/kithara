#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
mod backend;
mod entry;
mod fixtures;
#[cfg(feature = "stretch-identity")]
mod identity;
mod playback;
mod projection;
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
mod target;
mod timeline;

#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use std::num::NonZero;

use fixtures::{WarpRenderer, chunk, f64_of, render_serviced, renderer, spec};
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use fixtures::{dominant_bin, expected_bin, flush_serviced};
use kithara_platform::sync::Arc;
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_signal::AudioChunkInfo;
use kithara_signal::{OutputContext, SessionEpoch, SessionFrame};
use kithara_test_utils::kithara;

use crate::{PresentationFrontier, RenderContext, Warp, WarpConfig, test_pools::pools};
