use std::mem::size_of;

use kithara_apple::audio_toolbox::{AudioStreamBasicDescription, pod_from_prefix};
use kithara_stream::AudioCodec;

use super::super::{consts, converter::derive_aac_asbd_from_esds, flac};
use crate::{DecodeError, DecodeResult, demuxer::TrackInfo};

/// Computed Apple input-format triple — ASBD, frames-per-packet
/// estimate, optional magic cookie.
pub(super) struct AppleInputFormat {
    pub(super) asbd: AudioStreamBasicDescription,
    pub(super) cookie: Option<Box<[u8]>>,
    pub(super) frames_per_packet: u32,
}

/// Build the input ASBD + cookie + frames-per-packet for the given track.
pub(super) fn build_input_format(track: &TrackInfo) -> DecodeResult<AppleInputFormat> {
    match track.codec {
        AudioCodec::AacLc | AudioCodec::AacHe | AudioCodec::AacHeV2 => {
            build_aac_input_format(track)
        }
        AudioCodec::Flac => {
            let streaminfo = flac::streaminfo_body(&track.extra_data)?;
            let max_block = u32::from(u16::from_be_bytes([streaminfo[2], streaminfo[3]]))
                .max(consts::AAC_FRAMES_PER_PACKET);

            let mut cookie = [0; consts::FLAC_COOKIE_PREFIX_LEN + consts::FLAC_STREAMINFO_LEN];
            let cookie_size = u32::try_from(cookie.len())?;
            cookie[..4].copy_from_slice(&cookie_size.to_be_bytes());
            cookie[4..8].copy_from_slice(b"dfLa");
            cookie[12..16].copy_from_slice(&[0x80, 0x00, 0x00, consts::FLAC_STREAMINFO_LEN_U8]);
            cookie[consts::FLAC_COOKIE_PREFIX_LEN..].copy_from_slice(streaminfo);

            let asbd = AudioStreamBasicDescription {
                sample_rate: f64::from(track.sample_rate),
                format_id: consts::FORMAT_FLAC,
                frames_per_packet: max_block,
                channels_per_frame: u32::from(track.channels),
                ..Default::default()
            };
            Ok(AppleInputFormat {
                asbd,
                frames_per_packet: max_block,
                cookie: Some(Box::from(cookie)),
            })
        }
        AudioCodec::Pcm => {
            let asbd = parse_pcm_extra_data(&track.extra_data)?;
            Ok(AppleInputFormat {
                asbd,
                cookie: None,
                frames_per_packet: 1,
            })
        }
        AudioCodec::Mp3 => {
            let asbd = AudioStreamBasicDescription {
                sample_rate: f64::from(track.sample_rate),
                format_id: consts::FORMAT_MPEG_LAYER3,
                frames_per_packet: 0,
                channels_per_frame: u32::from(track.channels),
                ..Default::default()
            };
            Ok(AppleInputFormat {
                asbd,
                cookie: None,
                frames_per_packet: if track.sample_rate < 32_000 {
                    576
                } else {
                    1152
                },
            })
        }
        AudioCodec::Alac => {
            if track.extra_data.is_empty() {
                return Err(DecodeError::InvalidData {
                    detail: "alac: missing magic cookie (kAudioFilePropertyMagicCookieData)",
                });
            }
            let asbd = AudioStreamBasicDescription {
                sample_rate: f64::from(track.sample_rate),
                format_id: consts::FORMAT_APPLE_LOSSLESS,
                frames_per_packet: 0,
                channels_per_frame: u32::from(track.channels),
                ..Default::default()
            };
            Ok(AppleInputFormat {
                asbd,
                cookie: Some(track.extra_data.clone().into_boxed_slice()),
                frames_per_packet: 4096,
            })
        }
        other => Err(DecodeError::UnsupportedCodec { codec: other }),
    }
}

fn parse_pcm_extra_data(extra: &[u8]) -> DecodeResult<AudioStreamBasicDescription> {
    if extra.len() < size_of::<AudioStreamBasicDescription>() {
        return Err(DecodeError::InvalidData {
            detail: "pcm: extra_data too short for AudioStreamBasicDescription",
        });
    }
    pod_from_prefix(extra).ok_or(DecodeError::InvalidData {
        detail: "pcm: invalid AudioStreamBasicDescription payload",
    })
}

/// Build the AAC ASBD and ESDS cookie from Apple's richest `FormatList` entry.
/// Manual HE-AAC ASBD construction selects an LC pipeline, rejecting the cookie as `'!dat'`
/// and frames as `'bada'`. `FormatList` also rejects raw ASC, so wrap it in the minimum
/// ISO/IEC 14496-1 descriptor chain before discovery.
fn build_aac_input_format(track: &TrackInfo) -> DecodeResult<AppleInputFormat> {
    if track.extra_data.is_empty() {
        let asbd = AudioStreamBasicDescription {
            sample_rate: f64::from(track.sample_rate),
            format_id: consts::FORMAT_MPEG4_AAC,
            frames_per_packet: consts::AAC_FRAMES_PER_PACKET,
            channels_per_frame: u32::from(track.channels),
            ..Default::default()
        };
        return Ok(AppleInputFormat {
            asbd,
            cookie: None,
            frames_per_packet: consts::AAC_FRAMES_PER_PACKET,
        });
    }

    let esds = if track.extra_data.first() == Some(&0x03) {
        track.extra_data.clone()
    } else {
        esds_wrap_asc(&track.extra_data)?
    };
    let asbd = derive_aac_asbd_from_esds(&esds)?;
    let frames_per_packet = if asbd.frames_per_packet > 0 {
        asbd.frames_per_packet
    } else {
        consts::AAC_FRAMES_PER_PACKET
    };
    Ok(AppleInputFormat {
        asbd,
        frames_per_packet,
        cookie: Some(esds.into_boxed_slice()),
    })
}

/// Wrap a raw `AudioSpecificConfig` in the minimum ISO/IEC 14496-1 ESDS descriptor
/// chain Apple's `AudioFormat` / `AudioConverter` APIs accept as a magic cookie.
fn esds_wrap_asc(asc: &[u8]) -> DecodeResult<Vec<u8>> {
    const TOO_LONG: DecodeError = DecodeError::InvalidData {
        detail: "aac: descriptor too long for short-form ESDS size field",
    };
    let dsi_body: u8 = asc.len().try_into().map_err(|_| TOO_LONG)?;
    let dcd_body_len = 1 + 1 + 3 + 4 + 4 + 2 + asc.len();
    let dcd_body: u8 = dcd_body_len.try_into().map_err(|_| TOO_LONG)?;
    let esd_body_len = 2 + 1 + 2 + dcd_body_len + 3;
    let esd_body: u8 = esd_body_len.try_into().map_err(|_| TOO_LONG)?;

    let header: [u8; 22] = [
        0x03, esd_body, 0x00, 0x00, 0x00, 0x04, dcd_body, 0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, dsi_body,
    ];
    let trailer: [u8; 3] = [0x06, 0x01, 0x02];
    Ok(header
        .into_iter()
        .chain(asc.iter().copied())
        .chain(trailer)
        .collect())
}
