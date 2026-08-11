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
pub enum MpegVersion {
    /// Version 2.5
    Mpeg2p5,
    /// Version 2
    Mpeg2,
    /// Version 1
    Mpeg1,
}

/// The MPEG audio layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MpegLayer {
    /// Layer 1
    Layer1,
    /// Layer 2
    Layer2,
    /// Layer 3
    Layer3,
}

/// The channel mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    /// Single mono audio channel.
    Mono,
    /// Dual mono audio channels.
    DualMono,
    /// Stereo channels.
    Stereo,
    /// Joint Stereo encoded channels (decodes to Stereo).
    JointStereo,
}

impl ChannelMode {
    /// Gets the number of channels.
    #[inline(always)]
    pub fn count(&self) -> usize {
        match self {
            Self::Mono => 1,
            _ => 2,
        }
    }

    /// Gets the channel map.
    #[inline(always)]
    pub fn channels(&self) -> Channels {
        let positions = match self {
            Self::Mono => Position::FRONT_LEFT,
            _ => Position::FRONT_LEFT | Position::FRONT_RIGHT,
        };

        Channels::Positioned(positions)
    }
}

/// An MPEG 1, 2, or 2.5 audio frame header.
#[derive(Debug)]
pub struct FrameHeader {
    pub version: MpegVersion,
    pub layer: MpegLayer,
    pub sample_rate: u32,
    pub channel_mode: ChannelMode,
    pub has_crc: bool,
    pub frame_size: usize,
}

impl FrameHeader {
    /// Returns true if this is an MPEG-1 frame, false otherwise.
    #[inline(always)]
    pub fn is_mpeg1(&self) -> bool {
        self.version == MpegVersion::Mpeg1
    }

    /// Returns the codec ID for the frame.
    pub fn codec(&self) -> AudioCodecId {
        match self.layer {
            MpegLayer::Layer1 => CODEC_ID_MP1,
            MpegLayer::Layer2 => CODEC_ID_MP2,
            MpegLayer::Layer3 => CODEC_ID_MP3,
        }
    }

    /// Returns the number of per-channel audio samples in the MPEG frame.
    pub fn num_frames(&self) -> u16 {
        match self.layer {
            MpegLayer::Layer1 => 384,
            MpegLayer::Layer2 => 1152,
            MpegLayer::Layer3 => 576 * self.n_granules() as u16,
        }
    }

    /// Returns the duration of the MPEG frame.
    ///
    /// This is effectively the same as `num_frames`, but as a `Duration`.
    #[inline(always)]
    pub fn duration(&self) -> Duration {
        Duration::from(self.num_frames())
    }

    /// Returns the number of granules in the frame.
    #[inline(always)]
    pub fn n_granules(&self) -> usize {
        match self.version {
            MpegVersion::Mpeg1 => 2,
            _ => 1,
        }
    }

    /// Returns the number of channels per granule.
    #[inline(always)]
    pub fn n_channels(&self) -> usize {
        self.channel_mode.count()
    }

    /// Get the side information length.
    #[inline(always)]
    pub fn side_info_len(&self) -> usize {
        match (self.version, self.channel_mode) {
            (MpegVersion::Mpeg1, ChannelMode::Mono) => 17,
            (MpegVersion::Mpeg1, _) => 32,
            (_, ChannelMode::Mono) => 9,
            (_, _) => 17,
        }
    }
}
