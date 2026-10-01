#[cfg(test)]
use kithara_events::TrackId;
#[cfg(test)]
pub(crate) const BACKGROUND: TrackId = TrackId(9);

#[cfg(test)]
pub(crate) const OUTGOING: TrackId = TrackId(7);

#[cfg(test)]
pub(crate) const PROMOTED: TrackId = TrackId(8);

pub(crate) const DISCRIMINATOR_DOMAIN: &[u8] = b"kithara.play.query-discriminator.v1\0";
pub(crate) const HASH_BYTES: usize = 16;
pub(crate) const IDENTITY_DOMAIN: &[u8] = b"kithara.play.query-identity.v1\0";

#[cfg(test)]
pub(crate) const BLOCK_FRAMES: usize = 512;

#[cfg(test)]
pub(crate) const SAMPLE_RATE: u32 = 44_100;

pub(crate) const DEFAULT_EQ_BAND_COUNT: usize = 10;
pub(crate) const DEFAULT_PREFETCH_DURATION: f32 = 3.5;
pub(crate) const DEFAULT_MAX_SLOTS: usize = 4;

#[cfg(test)]
pub(crate) const DROPPED_AFTER_CANCEL: u8 = 2;

#[cfg(test)]
pub(crate) const DROPPED_BEFORE_CANCEL: u8 = 1;

#[cfg(test)]
pub(crate) const NOT_DROPPED: u8 = 0;
