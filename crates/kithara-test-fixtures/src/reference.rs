//! Prepared fixture PCM and independent audio references.

use std::io;

pub use kithara_fixture_media::reference::AudioReference;

use crate::{asset::Asset, assets::by_name};

/// Read the prepared independent PCM for a named whole audio asset.
///
/// # Errors
/// Returns an error when the dependent reference is unregistered, unavailable,
/// missing from the selected store, or malformed. Never invokes media tools.
pub fn audio(asset: &Asset) -> io::Result<AudioReference> {
    read("reference", asset)
}

/// Read authored fixture PCM with its original i16/32767 normalization.
/// # Errors
/// Returns an error if its prepared record is absent, unavailable or malformed.
pub fn authored(asset: &Asset) -> io::Result<AudioReference> {
    read("authored", asset)
}

/// Read the complete build-decoded input used by whole-track analysis.
/// # Errors
/// Returns an error if its prepared record is absent, unavailable or malformed.
pub fn source(asset: &Asset) -> io::Result<AudioReference> {
    read("source_pcm", asset)
}

fn read(prefix: &str, asset: &Asset) -> io::Result<AudioReference> {
    let name = format!("{prefix}_{}", asset.entry().name);
    let reference = by_name(&name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no prepared {prefix} PCM for {}", asset.entry().name),
        )
    })?;
    AudioReference::from_bytes(reference.try_bytes().map_err(io::Error::other)?)
}

/// The prepared three-second native source window used by MP3 clock checks.
/// # Panics
/// Panics if its prepared record is absent or malformed.
#[cfg(feature = "wav")]
#[must_use]
pub fn mp3_clock_window() -> AudioReference {
    AudioReference::from_bytes(crate::assets::authored_mp3_clock_default().bytes())
        .expect("prepared MP3 clock source window")
}

/// Exact source-authored beat and downbeat positions in the MP3 clock window.
/// # Panics
/// Panics if its prepared record is absent or malformed.
#[cfg(feature = "wav")]
#[must_use]
pub fn mp3_clock_markers() -> (Vec<usize>, Vec<usize>) {
    serde_json::from_slice(crate::assets::source_markers_mp3_clock_default().bytes())
        .expect("prepared MP3 source markers")
}
