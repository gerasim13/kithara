use kithara_bufpool::ByteBuffer;
use symphonia::core::{
    errors::Result,
    formats::{
        FormatReader, SeekMode, SeekTo, SeekedTo,
        well_known::{FORMAT_ID_MP1, FORMAT_ID_MP2, FORMAT_ID_MP3},
    },
    packet::Packet,
    units::{Duration, Timestamp},
};

use crate::error::{DecodeError, DecodeResult};

pub(super) struct PacketInfo {
    pub(super) track_id: u32,
    pub(super) pts: Timestamp,
    pub(super) dur: Duration,
}

pub(super) enum Packets {
    Borrowed {
        reader: Box<dyn FormatReader>,
        buffer: ByteBuffer,
        len: usize,
    },
    Owned {
        reader: Box<dyn FormatReader>,
        packet: Option<Packet>,
    },
}

impl Packets {
    pub(super) fn new(reader: Box<dyn FormatReader>, mut buffer: ByteBuffer) -> DecodeResult<Self> {
        if let Some(len) = reader.packet_buffer_size().map_err(DecodeError::backend)? {
            buffer.ensure_len(len)?;
            Ok(Self::Borrowed {
                reader,
                buffer,
                len: 0,
            })
        } else {
            Ok(Self::Owned {
                reader,
                packet: None,
            })
        }
    }

    pub(super) fn is_mpeg(&self) -> bool {
        matches!(
            match self {
                Self::Borrowed { reader, .. } | Self::Owned { reader, .. } =>
                    reader.format_info().format,
            },
            FORMAT_ID_MP1 | FORMAT_ID_MP2 | FORMAT_ID_MP3
        )
    }

    pub(super) fn read(&mut self) -> Result<Option<PacketInfo>> {
        match self {
            Self::Borrowed {
                reader,
                buffer,
                len,
            } => {
                *len = 0;
                Ok(reader.read_packet(buffer)?.map(|packet| {
                    *len = packet.data.len();
                    PacketInfo {
                        track_id: packet.track_id,
                        pts: packet.pts,
                        dur: packet.dur,
                    }
                }))
            }
            Self::Owned { reader, packet } => {
                *packet = reader.next_packet()?;
                Ok(packet.as_ref().map(|packet| PacketInfo {
                    track_id: packet.track_id,
                    pts: packet.pts,
                    dur: packet.dur,
                }))
            }
        }
    }

    pub(super) fn data(&self) -> &[u8] {
        match self {
            Self::Borrowed { buffer, len, .. } => &buffer[..*len],
            Self::Owned { packet, .. } => {
                &packet.as_ref().expect("packet was read successfully").data
            }
        }
    }

    pub(super) fn seek(&mut self, mode: SeekMode, to: SeekTo) -> Result<SeekedTo> {
        match self {
            Self::Borrowed { reader, buffer, .. } => reader.seek_with_buffer(mode, to, buffer),
            Self::Owned { reader, .. } => reader.seek(mode, to),
        }
    }
}
