//! Format-reader adapters shared by software decoding and native MPEG audio.
//! Native decoders use the in-tree MPEG demuxer with their own frame codec;
//! software codec registration and general source probing have separate owners.

pub(crate) mod adapter;
#[cfg(feature = "symphonia")]
mod codec;
pub(crate) mod demuxer;
mod error;
#[cfg(feature = "symphonia")]
mod open;
mod packets;
#[cfg(all(test, feature = "symphonia"))]
mod tests;
mod track;

#[cfg(feature = "symphonia")]
pub(crate) use codec::{SymphoniaCodec, SymphoniaConfig};
pub(crate) use demuxer::SymphoniaDemuxer;
#[cfg(feature = "symphonia")]
pub(crate) use open::FileOpen;
