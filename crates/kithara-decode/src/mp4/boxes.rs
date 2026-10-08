use std::io::ErrorKind;

use kithara_bufpool::{ByteBuffer, HasPool, PoolRegion};

use super::{
    Mp4MetadataError,
    parse::{invalid, read_be_u32, read_be_u64},
};
use crate::traits::DecoderInput;

/// Header information for one MP4 box, captured from a streaming reader.
#[derive(Clone, Copy)]
pub(super) struct BoxRef {
    pub(super) kind: [u8; 4],
    /// Absolute byte position one past the last byte of the box.
    pub(super) end: u64,
}

pub(super) fn next_box(
    reader: &mut dyn DecoderInput,
    end: Option<u64>,
) -> Result<Option<BoxRef>, Mp4MetadataError> {
    let start = reader.stream_position()?;
    if end.is_some_and(|limit| start >= limit) {
        return Ok(None);
    }

    let mut header = [0; 8];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
            if end.is_some() {
                return Err(invalid("truncated MP4 box header"));
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    }

    let size32 = read_be_u32(&header[..4]).ok_or_else(|| invalid("truncated MP4 box size"))?;
    let kind = [header[4], header[5], header[6], header[7]];

    let (header_len, total_size) = match size32 {
        1 => {
            let mut extended = [0; 8];
            reader.read_exact(&mut extended).map_err(|error| {
                if error.kind() == ErrorKind::UnexpectedEof {
                    invalid("truncated extended MP4 box size")
                } else {
                    error.into()
                }
            })?;
            let extended =
                read_be_u64(&extended).ok_or_else(|| invalid("invalid extended MP4 box size"))?;
            (16u64, extended)
        }
        0 => match end {
            Some(limit) => (8u64, limit - start),
            None => return Ok(None),
        },
        _ => (8u64, u64::from(size32)),
    };

    if total_size < header_len {
        return Err(invalid("MP4 box size is smaller than its header"));
    }

    let box_end = start
        .checked_add(total_size)
        .ok_or_else(|| invalid("MP4 box size overflow"))?;

    if end.is_some_and(|limit| box_end > limit) {
        return Err(invalid("MP4 box extends past available bytes"));
    }

    Ok(Some(BoxRef { kind, end: box_end }))
}

pub(super) fn read_payload<S>(
    reader: &mut dyn DecoderInput,
    end: u64,
    label: &str,
    pools: &PoolRegion<S>,
) -> Result<ByteBuffer, Mp4MetadataError>
where
    S: HasPool<u8>,
{
    let start = reader.stream_position()?;
    let payload_len = end
        .checked_sub(start)
        .and_then(|payload_len| usize::try_from(payload_len).ok())
        .ok_or_else(|| invalid(format!("{label} payload range underflow")))?;

    let mut payload = pools.get_with_len::<u8>(payload_len)?;
    reader.read_exact(&mut payload)?;
    Ok(payload)
}
