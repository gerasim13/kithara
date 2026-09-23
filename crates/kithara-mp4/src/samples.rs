use std::{
    io::{Seek, SeekFrom},
    ops::Range,
};

use re_mp4::{BoxHeader, BoxType, MoofBox, ReadBox, TfhdBox, TrunBox};

use crate::{
    cursor::{ReadAt, ReadAtCursor},
    error::Mp4Error,
};

/// One sample - a single access unit - inside a fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Byte range of the sample in the walked body. A body read through a
    /// [`ReadAt`] over one segment buffer makes these offsets into that
    /// buffer.
    pub byte_range: Range<u64>,
    /// Sample duration in media ticks.
    pub duration_ticks: u32,
    /// Absolute decode time in the track's media ticks.
    pub decode_ticks: u64,
}

/// Walk every `(moof, mdat)` pair of a body that carries no `moov` - an fMP4
/// media segment - and enumerate the samples of `track_id`.
///
/// A fragment that carries no `traf` for `track_id` falls back to its first
/// `traf`, which is how a single-track segment addresses itself.
///
/// # Errors
///
/// Returns [`Mp4Error`] when a box header or body does not parse, when a
/// `moof` is not followed by an `mdat`, or when a sample's byte range leaves
/// the body.
pub fn read_samples<R: ReadAt>(
    source: &R,
    total: u64,
    track_id: u32,
) -> Result<Vec<Sample>, Mp4Error> {
    /// Bytes of a plain box header, which is what a size must at least cover.
    const BOX_HEADER_BYTES: u64 = 8;

    let mut cursor = ReadAtCursor::new(source, total);
    let mut samples: Vec<Sample> = Vec::new();

    while position(&mut cursor)? < total {
        let box_start = position(&mut cursor)?;
        let header =
            BoxHeader::read(&mut cursor).map_err(|_| Mp4Error::new("unreadable box header"))?;
        if header.size < BOX_HEADER_BYTES {
            return Err(Mp4Error::new("invalid box size in media segment"));
        }
        let box_end = box_start.saturating_add(header.size);

        if header.name != BoxType::MoofBox {
            seek_to(&mut cursor, box_end)?;
            continue;
        }

        let moof = MoofBox::read_box(&mut cursor, header.size)
            .map_err(|_| Mp4Error::new("unreadable moof box"))?;
        seek_to(&mut cursor, box_end)?;

        let mdat_start = position(&mut cursor)?;
        let mdat = BoxHeader::read(&mut cursor)
            .map_err(|_| Mp4Error::new("unreadable box header after moof"))?;
        if mdat.name != BoxType::MdatBox {
            return Err(Mp4Error::new("expected mdat box after moof"));
        }
        seek_to(&mut cursor, mdat_start.saturating_add(mdat.size))?;

        collect_samples(&moof, box_start, track_id, total, &mut samples)?;
    }

    Ok(samples)
}

/// Append the samples one `moof` addresses, resolving each `trun`'s base
/// offset the way the `tfhd` flags ask for.
fn collect_samples(
    moof: &MoofBox,
    moof_start: u64,
    track_id: u32,
    total: u64,
    out: &mut Vec<Sample>,
) -> Result<(), Mp4Error> {
    let traf = moof
        .trafs
        .iter()
        .find(|traf| traf.tfhd.track_id == track_id)
        .or_else(|| moof.trafs.first())
        .ok_or_else(|| Mp4Error::new("moof has no traf"))?;

    let tfhd = &traf.tfhd;
    let default_base_is_moof = (tfhd.flags & TfhdBox::FLAG_DEFAULT_BASE_IS_MOOF) != 0;
    let mut decode_ticks = traf
        .tfdt
        .as_ref()
        .map_or(0, |tfdt| tfdt.base_media_decode_time);

    let sample_total: usize = traf
        .truns
        .iter()
        .map(|trun| usize::try_from(trun.sample_count).unwrap_or(0))
        .sum();
    out.reserve(sample_total);

    for trun in &traf.truns {
        let base = if default_base_is_moof {
            moof_start
        } else {
            tfhd.base_data_offset.unwrap_or(moof_start)
        };
        let mut byte_cursor = offset_from(base, trun.data_offset.unwrap_or(0));

        for index in 0..sample_count(trun) {
            let size = sample_size(trun, tfhd, index)?;
            let duration_ticks = sample_duration(trun, tfhd, index);
            let end = byte_cursor
                .checked_add(u64::from(size))
                .ok_or_else(|| Mp4Error::new("sample byte range overflow"))?;
            if end > total {
                return Err(Mp4Error::new("sample byte range past segment end"));
            }

            out.push(Sample {
                decode_ticks,
                duration_ticks,
                byte_range: byte_cursor..end,
            });

            byte_cursor = end;
            decode_ticks = decode_ticks.saturating_add(u64::from(duration_ticks));
        }
    }

    Ok(())
}

/// Samples a `trun` addresses, as a `usize` the sample loop can range over.
fn sample_count(trun: &TrunBox) -> usize {
    usize::try_from(trun.sample_count).unwrap_or(0)
}

/// Apply a `trun`'s signed data offset to its base.
fn offset_from(base: u64, data_offset: i32) -> u64 {
    if data_offset < 0 {
        base.saturating_sub(u64::from(data_offset.unsigned_abs()))
    } else {
        base.saturating_add(u64::from(data_offset.unsigned_abs()))
    }
}

/// Size of one sample: the `trun`'s own entry where it carries sizes, and the
/// `tfhd` default otherwise.
fn sample_size(trun: &TrunBox, tfhd: &TfhdBox, index: usize) -> Result<u32, Mp4Error> {
    if (trun.flags & TrunBox::FLAG_SAMPLE_SIZE) != 0 {
        return trun
            .sample_sizes
            .get(index)
            .copied()
            .ok_or_else(|| Mp4Error::new("missing trun sample_size"));
    }
    tfhd.default_sample_size
        .ok_or_else(|| Mp4Error::new("no default_sample_size"))
}

/// Duration of one sample, by the same rule as [`sample_size`].
fn sample_duration(trun: &TrunBox, tfhd: &TfhdBox, index: usize) -> u32 {
    if (trun.flags & TrunBox::FLAG_SAMPLE_DURATION) != 0 {
        return trun.sample_durations.get(index).copied().unwrap_or(0);
    }
    tfhd.default_sample_duration.unwrap_or(0)
}

fn position<R: ReadAt>(cursor: &mut ReadAtCursor<'_, R>) -> Result<u64, Mp4Error> {
    cursor
        .stream_position()
        .map_err(|_| Mp4Error::new("cannot read the walk position"))
}

fn seek_to<R: ReadAt>(cursor: &mut ReadAtCursor<'_, R>, offset: u64) -> Result<(), Mp4Error> {
    cursor
        .seek(SeekFrom::Start(offset))
        .map(|_| ())
        .map_err(|_| Mp4Error::new("cannot seek past a box"))
}

#[cfg(test)]
mod tests;
