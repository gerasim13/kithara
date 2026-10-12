#![cfg(feature = "mock")]

use std::ops::Range;

use kithara::{
    host::HostSettingsControl,
    output::{
        OfflineRenderError, OfflineRenderReport, OfflineRenderRequest, OfflineRenderer, RenderSink,
        RenderSinkError,
    },
    platform::CancelToken,
    signal::AudioSpec,
};
use kithara_test_utils::kithara as test_attrs;

use crate::pools::AppHost;

struct Discard;

impl RenderSink for Discard {
    fn write(&mut self, _: &[f32]) -> Result<(), RenderSinkError> {
        Ok(())
    }
}

/// Drive the application host through one exact offline output range.
///
/// # Errors
/// Returns the host's offline rendering failure.
#[test_attrs::flash(io)]
pub fn render_host(
    host: &mut AppHost,
    frames: Range<u64>,
    cancel: &CancelToken,
) -> Result<OfflineRenderReport, OfflineRenderError> {
    let request = OfflineRenderRequest::builder()
        .spec(AudioSpec::new(2, host.sample_rate()))
        .frames(frames)
        .build();
    host.render(&request, cancel, &mut Discard)
}
