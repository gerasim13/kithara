use kithara_beat::{BeatGridModel, RawBeatGrid};
use kithara_waveform::Waveform;

/// A prepared artifact read back from the bytes of its own document.
///
/// Each artifact kind carries its own format, chosen by whoever owns the
/// artifact: there is no probing and no guessing chain. A loader asks the kind
/// it wants for a document, and the kind either recognises the bytes or names
/// why it does not.
pub trait ArtifactDocument: Sized {
    /// What this artifact is called in an error a caller reads.
    const KIND: &'static str;

    /// Read the artifact out of one whole document.
    ///
    /// # Errors
    ///
    /// Returns why the bytes are not a document of this kind.
    fn decode(bytes: &[u8]) -> Result<Self, String>;
}

impl ArtifactDocument for BeatGridModel {
    const KIND: &'static str = "beat grid";

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let raw: RawBeatGrid = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        Self::try_from(raw).map_err(|error| error.to_string())
    }
}

impl ArtifactDocument for Waveform {
    const KIND: &'static str = "waveform";

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Self::try_from(bytes).map_err(|error| error.to_string())
    }
}
