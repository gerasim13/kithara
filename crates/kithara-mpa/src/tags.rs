// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use symphonia_core::{
    checksum::Crc16AnsiLe,
    errors::Result,
    io::{BufReader, FiniteStream, Monitor, MonitorStream, ReadBytes},
};
use tracing::{info, warn};

use crate::{
    common::{FrameHeader, MpegLayer},
    header::MPEG_HEADER_LEN,
};

struct TagIds;

impl TagIds {
    const INFO: [u8; 4] = *b"Info";
    const LEN: usize = 4;
    const VBRI: [u8; 4] = *b"VBRI";
    const XING: [u8; 4] = *b"Xing";
}

struct XingLayout;

impl XingLayout {
    const BYTES_FLAG: u32 = 0x2;
    const QUALITY_FLAG: u32 = 0x8;
    const TOC_FLAG: u32 = 0x4;
    const TOC_LEN: usize = 100;
}

struct LameLayout;

impl LameLayout {
    const DECODER_DELAY: u32 = 529;
    const ENCODER_ID_LEN: usize = 4;
    const ENCODER_LEN: usize = 9;
    const TRIM_BITS: u32 = 12;
}

const VBRI_TAG_OFFSET: usize = 36;

/// The LAME tag is an extension to the Xing/Info tag.
pub(crate) struct LameTag {
    pub(crate) enc_delay: u32,
    pub(crate) enc_padding: u32,
}

/// Timing information from a Xing/Info tag in an MP3 file.
pub(crate) struct XingInfoTag {
    pub(crate) lame: Option<LameTag>,
    pub(crate) num_frames: Option<u32>,
}

/// Try to read a Xing/Info tag from the provided MPEG frame.
pub(crate) fn try_read_info_tag(buf: &[u8], header: &FrameHeader) -> Option<XingInfoTag> {
    try_read_info_tag_inner(buf, header).ok().flatten()
}

fn try_read_info_tag_inner(buf: &[u8], header: &FrameHeader) -> Result<Option<XingInfoTag>> {
    /// The full LAME extension size.
    const LAME_EXT_LEN: u64 = 36;
    /// The minimal LAME extension size up to the encoding delay and padding fields.
    const MIN_LAME_EXT_LEN: u64 = 24;

    if !is_maybe_info_tag(buf, header) {
        return Ok(None);
    }

    let offset = header.side_info_len();

    let mut crc16 = Crc16AnsiLe::new(0);
    crc16.process_buf_bytes(&buf[..offset + MPEG_HEADER_LEN]);

    let mut reader = MonitorStream::new(BufReader::new(&buf[offset + MPEG_HEADER_LEN..]), crc16);

    let id = reader.read_quad_bytes()?;

    if id != TagIds::XING && id != TagIds::INFO {
        return Ok(None);
    }

    let flags = reader.read_be_u32()?;

    let num_frames = if flags & 0x1 != 0 {
        Some(reader.read_be_u32()?)
    } else {
        None
    };

    if flags & XingLayout::BYTES_FLAG != 0 {
        let _num_bytes = reader.read_be_u32()?;
    }

    if flags & XingLayout::TOC_FLAG != 0 {
        let mut toc = [0; XingLayout::TOC_LEN];
        reader.read_buf_exact(&mut toc)?;
    }

    if flags & XingLayout::QUALITY_FLAG != 0 {
        let _quality = reader.read_be_u32()?;
    }

    let lame = if reader.inner().bytes_available() >= MIN_LAME_EXT_LEN {
        let mut encoder = [0; LameLayout::ENCODER_LEN];
        reader.read_buf_exact(&mut encoder)?;

        let _revision = reader.read_u8()?;

        let _lowpass = reader.read_u8()?;

        let _replaygain_peak = reader.read_be_u32()?;
        let _replaygain_radio = reader.read_be_u16()?;
        let _replaygain_audiophile = reader.read_be_u16()?;

        let _encoding_flags = reader.read_u8()?;

        let _abr = reader.read_u8()?;

        let (enc_delay, enc_padding) = {
            let trim = reader.read_be_u24()?;

            if encoder[..LameLayout::ENCODER_ID_LEN] == *b"LAME"
                || encoder[..LameLayout::ENCODER_ID_LEN] == *b"Lavf"
                || encoder[..LameLayout::ENCODER_ID_LEN] == *b"Lavc"
            {
                let delay = LameLayout::DECODER_DELAY + (trim >> LameLayout::TRIM_BITS);
                let padding = trim & ((1 << LameLayout::TRIM_BITS) - 1);

                (delay, padding.saturating_sub(LameLayout::DECODER_DELAY))
            } else {
                (0, 0)
            }
        };

        let crc = if reader.inner().bytes_available() >= LAME_EXT_LEN - MIN_LAME_EXT_LEN {
            let _misc = reader.read_u8()?;

            let _mp3_gain = reader.read_u8()?;

            let _surround_info = reader.read_be_u16()?;

            let _music_len = reader.read_be_u32()?;

            let _music_crc = reader.read_be_u16()?;

            if header.has_crc || encoder[..LameLayout::ENCODER_ID_LEN] == *b"LAME" {
                // WHY: The stored CRC is not part of the checksum it validates.
                Some(reader.inner_mut().read_be_u16()?)
            } else {
                None
            }
        } else {
            info!("xing tag lame extension is truncated");
            None
        };

        let is_tag_ok = crc.is_none_or(|crc| crc == 0 || crc == reader.monitor().crc());

        if is_tag_ok {
            Some(LameTag {
                enc_delay,
                enc_padding,
            })
        } else {
            warn!("xing tag lame extension crc mismatch");
            None
        }
    } else {
        info!("xing tag too small for lame extension");
        None
    };

    Ok(Some(XingInfoTag { lame, num_frames }))
}

/// Perform a fast check to see if the packet contains a Xing/Info tag. If this returns true, the
/// packet should be parsed fully to ensure it is in fact a tag.
pub(crate) fn is_maybe_info_tag(buf: &[u8], header: &FrameHeader) -> bool {
    const MIN_XING_TAG_LEN: usize = 8;

    if header.layer != MpegLayer::Layer3 {
        return false;
    }

    let offset = header.side_info_len() + MPEG_HEADER_LEN;

    if buf.len() < offset + MIN_XING_TAG_LEN {
        return false;
    }

    let id = &buf[offset..offset + TagIds::LEN];

    if id != TagIds::XING && id != TagIds::INFO {
        return false;
    }

    !buf[MPEG_HEADER_LEN..offset].iter().any(|&b| b != 0)
}

/// The contents of a VBRI tag.
pub(crate) struct VbriTag {
    pub(crate) num_mpeg_frames: u32,
}

/// Try to read a VBRI tag from the provided MPEG frame.
pub(crate) fn try_read_vbri_tag(buf: &[u8], header: &FrameHeader) -> Option<VbriTag> {
    try_read_vbri_tag_inner(buf, header).ok().flatten()
}

fn try_read_vbri_tag_inner(buf: &[u8], header: &FrameHeader) -> Result<Option<VbriTag>> {
    if !is_maybe_vbri_tag(buf, header) {
        return Ok(None);
    }

    let mut reader = BufReader::new(buf);

    reader.ignore_bytes(VBRI_TAG_OFFSET as u64)?;

    let id = reader.read_quad_bytes()?;

    if id != TagIds::VBRI {
        return Ok(None);
    }

    let version = reader.read_be_u16()?;

    if version != 1 {
        return Ok(None);
    }

    let _delay = reader.read_be_u16()?;
    let _quality = reader.read_be_u16()?;

    let _num_bytes = reader.read_be_u32()?;
    let num_mpeg_frames = reader.read_be_u32()?;

    Ok(Some(VbriTag { num_mpeg_frames }))
}

/// Perform a fast check to see if the packet contains a VBRI tag. If this returns true, the
/// packet should be parsed fully to ensure it is in fact a tag.
pub(crate) fn is_maybe_vbri_tag(buf: &[u8], header: &FrameHeader) -> bool {
    const MIN_VBRI_TAG_LEN: usize = 26;
    if header.layer != MpegLayer::Layer3 {
        return false;
    }

    if buf.len() < VBRI_TAG_OFFSET + MIN_VBRI_TAG_LEN {
        return false;
    }

    let id = &buf[VBRI_TAG_OFFSET..VBRI_TAG_OFFSET + TagIds::LEN];

    if id != TagIds::VBRI {
        return false;
    }

    !buf[MPEG_HEADER_LEN..VBRI_TAG_OFFSET]
        .iter()
        .any(|&b| b != 0)
}
