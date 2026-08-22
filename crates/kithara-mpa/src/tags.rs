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
    const VBRI: [u8; 4] = *b"VBRI";
    const XING: [u8; 4] = *b"Xing";
}

/// The LAME tag is an extension to the Xing/Info tag.
pub(crate) struct LameTag {
    pub(crate) enc_delay: u32,
    pub(crate) enc_padding: u32,
}

/// Timing information from a Xing/Info tag in an MP3 file.
pub(crate) struct XingInfoTag {
    pub(crate) num_frames: Option<u32>,
    pub(crate) lame: Option<LameTag>,
}

/// Try to read a Xing/Info tag from the provided MPEG frame.
pub(crate) fn try_read_info_tag(buf: &[u8], header: &FrameHeader) -> Option<XingInfoTag> {
    // The Info header is a completely optional piece of information. Therefore, flatten an error
    // reading the tag into a None.
    try_read_info_tag_inner(buf, header).ok().flatten()
}

fn try_read_info_tag_inner(buf: &[u8], header: &FrameHeader) -> Result<Option<XingInfoTag>> {
    /// The full LAME extension size.
    const LAME_EXT_LEN: u64 = 36;
    /// The minimal LAME extension size up to the encoding delay and padding fields.
    const MIN_LAME_EXT_LEN: u64 = 24;

    // Do a quick check that this is a Xing/Info tag.
    if !is_maybe_info_tag(buf, header) {
        return Ok(None);
    }

    // The position of the Xing/Info tag relative to the end of the header. This is equal to the
    // side information length for the frame.
    let offset = header.side_info_len();

    // Start the CRC with the header and side information.
    let mut crc16 = Crc16AnsiLe::new(0);
    crc16.process_buf_bytes(&buf[..offset + MPEG_HEADER_LEN]);

    // Start reading the Xing/Info tag after the side information.
    let mut reader = MonitorStream::new(BufReader::new(&buf[offset + MPEG_HEADER_LEN..]), crc16);

    // Check for Xing/Info header.
    let id = reader.read_quad_bytes()?;

    if id != TagIds::XING && id != TagIds::INFO {
        return Ok(None);
    }

    // Flags indicates what information is provided in this Xing/Info tag.
    let flags = reader.read_be_u32()?;

    let num_frames = if flags & 0x1 != 0 {
        Some(reader.read_be_u32()?)
    } else {
        None
    };

    if flags & 0x2 != 0 {
        let _num_bytes = reader.read_be_u32()?;
    }

    if flags & 0x4 != 0 {
        let mut toc = [0; 100];
        reader.read_buf_exact(&mut toc)?;
    }

    if flags & 0x8 != 0 {
        let _quality = reader.read_be_u32()?;
    }

    // The LAME extension may not always be present, or complete. The important fields in the
    // extension are within the first 24 bytes. Therefore, try to read those if they're available.
    let lame = if reader.inner().bytes_available() >= MIN_LAME_EXT_LEN {
        // Encoder string.
        let mut encoder = [0; 9];
        reader.read_buf_exact(&mut encoder)?;

        // Revision.
        let _revision = reader.read_u8()?;

        // Lowpass filter value.
        let _lowpass = reader.read_u8()?;

        let _replaygain_peak = reader.read_be_u32()?;
        let _replaygain_radio = reader.read_be_u16()?;
        let _replaygain_audiophile = reader.read_be_u16()?;

        // Encoding flags & ATH type.
        let _encoding_flags = reader.read_u8()?;

        // Arbitrary bitrate.
        let _abr = reader.read_u8()?;

        let (enc_delay, enc_padding) = {
            let trim = reader.read_be_u24()?;

            if encoder[..4] == *b"LAME" || encoder[..4] == *b"Lavf" || encoder[..4] == *b"Lavc" {
                let delay = 528 + 1 + (trim >> 12);
                let padding = trim & ((1 << 12) - 1);

                (delay, padding.saturating_sub(528 + 1))
            } else {
                (0, 0)
            }
        };

        // If possible, attempt to read the extra fields of the extension if they weren't
        // truncated.
        let crc = if reader.inner().bytes_available() >= LAME_EXT_LEN - MIN_LAME_EXT_LEN {
            // Flags.
            let _misc = reader.read_u8()?;

            // MP3 gain.
            let _mp3_gain = reader.read_u8()?;

            // Preset and surround info.
            let _surround_info = reader.read_be_u16()?;

            // Music length.
            let _music_len = reader.read_be_u32()?;

            // Music (audio) CRC.
            let _music_crc = reader.read_be_u16()?;

            // The tag CRC. LAME always includes this CRC regardless of the protection bit, but
            // other encoders may only do so if the protection bit is set.
            if header.has_crc || encoder[..4] == *b"LAME" {
                // Read the CRC using the inner reader to not change the computed CRC.
                Some(reader.inner_mut().read_be_u16()?)
            } else {
                // No CRC is present.
                None
            }
        } else {
            // The tag is truncated. No CRC will be present.
            info!("xing tag lame extension is truncated");
            None
        };

        // If there was no CRC written, then assume the tag is correct. Otherwise, use the CRC.
        // Accept a written CRC of 0, which de facto means to ignore the CRC.
        let is_tag_ok = crc.is_none_or(|crc| crc == 0 || crc == reader.monitor().crc());

        if is_tag_ok {
            // The CRC matched or is not present.
            Some(LameTag {
                enc_delay,
                enc_padding,
            })
        } else {
            // The CRC did not match, this is probably not a LAME tag.
            warn!("xing tag lame extension crc mismatch");
            None
        }
    } else {
        // Frame not large enough for a LAME tag.
        info!("xing tag too small for lame extension");
        None
    };

    Ok(Some(XingInfoTag { num_frames, lame }))
}

/// Perform a fast check to see if the packet contains a Xing/Info tag. If this returns true, the
/// packet should be parsed fully to ensure it is in fact a tag.
pub(crate) fn is_maybe_info_tag(buf: &[u8], header: &FrameHeader) -> bool {
    const MIN_XING_TAG_LEN: usize = 8;

    // Only supported with layer 3 packets.
    if header.layer != MpegLayer::Layer3 {
        return false;
    }

    // The position of the Xing/Info tag relative to the start of the packet. This is equal to the
    // side information length for the frame.
    let offset = header.side_info_len() + MPEG_HEADER_LEN;

    // The packet must be big enough to contain a tag.
    if buf.len() < offset + MIN_XING_TAG_LEN {
        return false;
    }

    // The tag ID must be present and correct.
    let id = &buf[offset..offset + 4];

    if id != TagIds::XING && id != TagIds::INFO {
        return false;
    }

    // The side information should be zeroed.
    !buf[MPEG_HEADER_LEN..offset].iter().any(|&b| b != 0)
}

/// The contents of a VBRI tag.
pub(crate) struct VbriTag {
    pub(crate) num_mpeg_frames: u32,
}

/// Try to read a VBRI tag from the provided MPEG frame.
pub(crate) fn try_read_vbri_tag(buf: &[u8], header: &FrameHeader) -> Option<VbriTag> {
    // The VBRI header is a completely optional piece of information. Therefore, flatten an error
    // reading the tag into a None.
    try_read_vbri_tag_inner(buf, header).ok().flatten()
}

fn try_read_vbri_tag_inner(buf: &[u8], header: &FrameHeader) -> Result<Option<VbriTag>> {
    // Do a quick check that this is a VBRI tag.
    if !is_maybe_vbri_tag(buf, header) {
        return Ok(None);
    }

    let mut reader = BufReader::new(buf);

    // The VBRI tag is always 32 bytes after the header.
    reader.ignore_bytes(MPEG_HEADER_LEN as u64 + 32)?;

    // Check for the VBRI signature.
    let id = reader.read_quad_bytes()?;

    if id != TagIds::VBRI {
        return Ok(None);
    }

    // The version is always 1.
    let version = reader.read_be_u16()?;

    if version != 1 {
        return Ok(None);
    }

    // Delay is a two-byte big-endian floating-point value.
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
    const VBRI_TAG_OFFSET: usize = 36;

    // Only supported with layer 3 packets.
    if header.layer != MpegLayer::Layer3 {
        return false;
    }

    // The packet must be big enough to contain a tag.
    if buf.len() < VBRI_TAG_OFFSET + MIN_VBRI_TAG_LEN {
        return false;
    }

    // The tag ID must be present and correct.
    let id = &buf[VBRI_TAG_OFFSET..VBRI_TAG_OFFSET + 4];

    if id != TagIds::VBRI {
        return false;
    }

    // The bytes preceding the VBRI tag (mostly the side information) should be all 0.
    !buf[MPEG_HEADER_LEN..VBRI_TAG_OFFSET]
        .iter()
        .any(|&b| b != 0)
}
