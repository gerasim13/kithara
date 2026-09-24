use std::ops::Range;

use re_mp4::Mp4;

use crate::cursor::{ReadAt, ReadAtCursor};

/// One `moof` + `mdat` fragment.
///
/// Times are media ticks against [`Fmp4Layout::timescale`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    /// Byte range the fragment occupies: its `moof` start up to the next
    /// `moof`, or to the end of the file for the last fragment.
    pub byte_range: Range<u64>,
    /// Decode time of the fragment's first sample, from `tfdt` where the
    /// fragment carries one and accumulated from its predecessors otherwise.
    pub decode_ticks: u64,
    /// Summed duration of the fragment's samples.
    pub duration_ticks: u64,
}

/// Fragment layout of a fragmented-mp4 file.
#[derive(Clone, Debug)]
pub struct Fmp4Layout {
    init_range: Range<u64>,
    fragments: Vec<Fragment>,
    timescale: u32,
}

impl Fmp4Layout {
    /// Fragments in file order.
    #[must_use]
    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }

    /// Byte range of the initialisation segment: everything before the first
    /// `moof`.
    #[must_use]
    pub fn init_range(&self) -> Range<u64> {
        self.init_range.clone()
    }

    /// Walk the box headers of the `total`-byte file behind `source` and
    /// derive its fragment layout. Payload boxes are seeked over, so peak
    /// memory tracks the layout, not the track length.
    ///
    /// Returns `None` when:
    /// - the bytes do not parse as mp4,
    /// - the mp4 has no `moof` boxes (a classic, non-fragmented file),
    /// - the audio track's timescale is unavailable or zero,
    /// - the first `moof` starts at byte zero, leaving no init segment,
    /// - a fragment is malformed: no `traf`, zero duration, or a byte range
    ///   that does not advance.
    pub fn read<R: ReadAt>(source: &R, total: u64) -> Option<Self> {
        let mp4 = parse(source, total)?;
        let timescale = timescale_of(&mp4, total)?;
        let first_moof_start = init_end(&mp4, total)?;
        let fragments = fragments_of(&mp4, total)?;

        Some(Self {
            timescale,
            fragments,
            init_range: 0..first_moof_start,
        })
    }

    /// Media timescale of the audio track, in ticks per second.
    #[must_use]
    pub const fn timescale(&self) -> u32 {
        self.timescale
    }
}

/// Walk the box headers into a parsed mp4, rejecting bytes that are not a
/// fragmented mp4 at all.
fn parse<R: ReadAt>(source: &R, total: u64) -> Option<Mp4> {
    let mp4 = match Mp4::read(ReadAtCursor::new(source, total), total) {
        Ok(mp4) => mp4,
        Err(error) => {
            tracing::debug!(%error, total, "mp4 box walk found no parsable mp4");
            return None;
        }
    };
    if mp4.moofs.is_empty() {
        tracing::debug!(total, "mp4 has no moof chain; not a fragmented file");
        return None;
    }
    Some(mp4)
}

/// Timescale every tick in the layout is measured against.
fn timescale_of(mp4: &Mp4, total: u64) -> Option<u32> {
    let Some(timescale) = audio_track_timescale(mp4).filter(|scale| *scale != 0) else {
        tracing::warn!(total, "fragmented mp4 carries no usable audio timescale");
        return None;
    };
    Some(timescale)
}

/// Where the init segment ends: the start of the first `moof`.
fn init_end(mp4: &Mp4, total: u64) -> Option<u64> {
    let first_moof_start = mp4.moofs.first()?.start;
    if first_moof_start == 0 {
        tracing::warn!(
            total,
            "first moof starts at byte zero, leaving no init segment"
        );
        return None;
    }
    Some(first_moof_start)
}

/// Every fragment in file order. A single malformed fragment voids the
/// layout: a seek index with a hole in it is worse than none.
fn fragments_of(mp4: &Mp4, total: u64) -> Option<Vec<Fragment>> {
    let mut fragments: Vec<Fragment> = Vec::with_capacity(mp4.moofs.len());
    let mut prev_decode_ticks: Option<u64> = None;

    for (idx, moof) in mp4.moofs.iter().enumerate() {
        let byte_end = mp4.moofs.get(idx + 1).map_or(total, |next| next.start);
        let Some(fragment) = fragment_from_moof(moof, byte_end, prev_decode_ticks) else {
            tracing::warn!(
                fragment_index = idx,
                moof_start = moof.start,
                byte_end,
                "malformed fragment; the file yields no layout"
            );
            return None;
        };
        prev_decode_ticks = Some(
            fragment
                .decode_ticks
                .saturating_add(fragment.duration_ticks),
        );
        fragments.push(fragment);
    }

    Some(fragments)
}

/// Build one fragment from its `moof`, carrying `prev_decode_ticks` in for a
/// fragment whose `tfdt` is absent.
fn fragment_from_moof(
    moof: &re_mp4::MoofBox,
    byte_end: u64,
    prev_decode_ticks: Option<u64>,
) -> Option<Fragment> {
    let traf = moof.trafs.first()?;
    let decode_ticks = traf
        .tfdt
        .as_ref()
        .map(|tfdt| tfdt.base_media_decode_time)
        .or(prev_decode_ticks)
        .unwrap_or(0);
    let duration_ticks: u64 = traf
        .truns
        .iter()
        .map(|trun| trun_duration_ticks(trun, &traf.tfhd))
        .sum();
    if duration_ticks == 0 {
        return None;
    }
    let byte_start = moof.start;
    if byte_end <= byte_start {
        return None;
    }
    Some(Fragment {
        decode_ticks,
        duration_ticks,
        byte_range: byte_start..byte_end,
    })
}

/// Total sample duration a `trun` contributes: its own per-sample durations
/// where it carries them, and the `tfhd` default repeated per sample
/// otherwise.
fn trun_duration_ticks(trun: &re_mp4::TrunBox, tfhd: &re_mp4::TfhdBox) -> u64 {
    if !trun.sample_durations.is_empty() {
        return trun
            .sample_durations
            .iter()
            .map(|duration| u64::from(*duration))
            .sum::<u64>();
    }
    if trun.sample_count > 0 {
        let default_duration = tfhd.default_sample_duration.unwrap_or(0);
        return u64::from(trun.sample_count).saturating_mul(u64::from(default_duration));
    }
    0
}

/// Timescale of the first track whose sample entry reads as audio. An
/// unknown sample entry counts: the timescale comes from `mdhd`, so the
/// codec box itself never has to be decodable.
fn audio_track_timescale(mp4: &Mp4) -> Option<u32> {
    mp4.moov
        .traks
        .iter()
        .find(|trak| {
            matches!(
                trak.mdia.minf.stbl.stsd.contents,
                re_mp4::StsdBoxContent::Mp4a(_) | re_mp4::StsdBoxContent::Unknown(_)
            )
        })
        .map(|trak| trak.mdia.mdhd.timescale)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
