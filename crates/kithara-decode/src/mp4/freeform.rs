use std::io::SeekFrom;

use kithara_bufpool::{HasPool, PoolRegion};
use smallvec::SmallVec;

use super::{
    ItunSmpb, Mp4MetadataError,
    boxes::{next_box, read_payload},
    parse::{parse_data_box, parse_itunsmpb, read_text_fullbox_bytes},
};
use crate::{consts, traits::DecoderInput};

/// Read only the bounded iTunes gapless tag; unrelated data is never allocated.
pub(super) fn read_itunsmpb<S>(
    reader: &mut dyn DecoderInput,
    end: u64,
    pools: &PoolRegion<S>,
) -> Result<Option<ItunSmpb>, Mp4MetadataError>
where
    S: HasPool<u8>,
{
    let start = reader.stream_position()?;
    let payload_len = end.saturating_sub(start);
    let too_big = usize::try_from(payload_len).map_or(true, |len| len > consts::FREEFORM_MAX_BYTES);
    if payload_len == 0 || too_big {
        return Ok(None);
    }

    let mut mean: Option<SmallVec<[u8; 32]>> = None;
    let mut name: Option<SmallVec<[u8; 32]>> = None;
    let mut data_range: Option<(u64, u64)> = None;

    while let Some(header) = next_box(reader, Some(end))? {
        match header.kind {
            consts::BOX_MEAN => {
                let payload = read_payload(reader, header.end, "mean", pools)?;
                mean = read_text_fullbox_bytes(&payload);
            }
            consts::BOX_NAME => {
                let payload = read_payload(reader, header.end, "name", pools)?;
                name = read_text_fullbox_bytes(&payload);
            }
            consts::BOX_DATA => data_range = Some((reader.stream_position()?, header.end)),
            _ => {}
        }
        reader.seek(SeekFrom::Start(header.end))?;
    }

    let matches = mean
        .as_deref()
        .is_some_and(|m| m == consts::ITUNES_MEAN.as_bytes())
        && name
            .as_deref()
            .is_some_and(|n| n == consts::ITUNSMPB_NAME.as_bytes());
    if !matches {
        return Ok(None);
    }

    let Some((data_start, data_end)) = data_range else {
        return Ok(None);
    };

    reader.seek(SeekFrom::Start(data_start))?;
    let payload = read_payload(reader, data_end, "iTunSMPB data", pools)?;
    let Some((_data_type, value)) = parse_data_box(&payload) else {
        return Ok(None);
    };
    let Some(info) = parse_itunsmpb(value) else {
        return Ok(None);
    };

    Ok(Some(info))
}
