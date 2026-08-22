// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use symphonia_core::{
    audio::{Channels, Position},
    codecs::audio::{
        AudioCodecId,
        well_known::{CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3},
    },
    units::Duration,
};

/// The MPEG audio version.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum MpegVersion {
    /// Version 2.5
    Mpeg2p5,
    /// Version 2
    Mpeg2,
    /// Version 1
    Mpeg1,
}

/// The MPEG audio layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum MpegLayer {
    /// Layer 1
    Layer1,
    /// Layer 2
    Layer2,
    /// Layer 3
    Layer3,
}

/// The channel mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChannelMode {
    /// Single mono audio channel.
    Mono,
    /// Dual mono audio channels.
    DualMono,
    /// Stereo channels.
    Stereo,
    /// Joint Stereo encoded channels (decodes to Stereo).
    JointStereo,
}

impl From<ChannelMode> for usize {
    /// The number of channels the mode decodes to.
    #[inline(always)]
    fn from(mode: ChannelMode) -> Self {
        match mode {
            ChannelMode::Mono => 1,
            _ => 2,
        }
    }
}

impl ChannelMode {
    /// Gets the channel map.
    #[inline(always)]
    pub(crate) fn channels(self) -> Channels {
        let positions = match self {
            Self::Mono => Position::FRONT_LEFT,
            _ => Position::FRONT_LEFT | Position::FRONT_RIGHT,
        };

        Channels::Positioned(positions)
    }
}

/// An MPEG 1, 2, or 2.5 audio frame header.
#[derive(Debug)]
pub(crate) struct FrameHeader {
    pub(crate) version: MpegVersion,
    pub(crate) layer: MpegLayer,
    pub(crate) sample_rate: u32,
    pub(crate) channel_mode: ChannelMode,
    pub(crate) has_crc: bool,
    pub(crate) frame_size: usize,
}

impl FrameHeader {
    /// Returns true if this is an MPEG-1 frame, false otherwise.
    #[inline(always)]
    pub(crate) fn is_mpeg1(&self) -> bool {
        self.version == MpegVersion::Mpeg1
    }

    /// Returns the codec ID for the frame.
    pub(crate) fn codec(&self) -> AudioCodecId {
        match self.layer {
            MpegLayer::Layer1 => CODEC_ID_MP1,
            MpegLayer::Layer2 => CODEC_ID_MP2,
            MpegLayer::Layer3 => CODEC_ID_MP3,
        }
    }

    /// Returns the number of per-channel audio samples in the MPEG frame.
    pub(crate) fn num_frames(&self) -> u16 {
        match self.layer {
            MpegLayer::Layer1 => 384,
            MpegLayer::Layer2 => 1152,
            MpegLayer::Layer3 => 576 * self.n_granules(),
        }
    }

    /// Returns the duration of the MPEG frame.
    ///
    /// This is effectively the same as `num_frames`, but as a `Duration`.
    #[inline(always)]
    pub(crate) fn duration(&self) -> Duration {
        Duration::from(self.num_frames())
    }

    /// Returns the number of granules in the frame.
    #[inline(always)]
    pub(crate) fn n_granules(&self) -> u16 {
        match self.version {
            MpegVersion::Mpeg1 => 2,
            _ => 1,
        }
    }

    /// Returns the number of channels per granule.
    #[inline(always)]
    pub(crate) fn n_channels(&self) -> usize {
        usize::from(self.channel_mode)
    }

    /// Get the side information length.
    #[inline(always)]
    pub(crate) fn side_info_len(&self) -> usize {
        match (self.version, self.channel_mode) {
            (MpegVersion::Mpeg1, ChannelMode::Mono) => 17,
            (MpegVersion::Mpeg1, _) => 32,
            (_, ChannelMode::Mono) => 9,
            (_, _) => 17,
        }
    }
}
