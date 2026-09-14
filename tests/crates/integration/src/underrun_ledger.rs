//! Starved renders a capture observed, read at their own output frames.

use std::ops::Range;

use kithara_test_utils::probe::capture::ProbeEvent;
use serde::Serialize;

/// One `pcm_underrun` probe with the fields a silence interval needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UnderrunEvent {
    pub track_id: Option<u64>,
    pub output_start: u64,
    pub requested_frames: u64,
    pub available_frames: u64,
    pub source_end: Option<u64>,
    pub warp_map_revision: Option<u64>,
    pub seq: Option<u64>,
}

impl UnderrunEvent {
    const PROBE: &'static str = "pcm_underrun";

    /// Output frames the feeder filled with silence.
    #[must_use]
    pub fn silence(&self) -> Range<u64> {
        let start = self
            .output_start
            .saturating_add(self.available_frames.min(self.requested_frames));
        start..self.output_start.saturating_add(self.requested_frames)
    }

    fn from_probe(probe: &ProbeEvent) -> Option<Self> {
        Some(Self {
            track_id: probe.u64("track_id"),
            output_start: probe.u64("output_start")?,
            requested_frames: probe.u64("requested_frames")?,
            available_frames: probe.u64("available_frames")?,
            source_end: probe.u64("source_end"),
            warp_map_revision: probe.u64("warp_map_revision"),
            seq: probe.seq(),
        })
    }
}

/// One track's starvation over a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UnderrunTrack {
    /// `None` gathers probes that carried no track.
    pub track_id: Option<u64>,
    pub underruns: u64,
    pub silenced_frames: u64,
    pub first_output_frame: u64,
    pub last_output_frame: u64,
}

/// Every starved render a capture observed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UnderrunLedger {
    pub events: Vec<UnderrunEvent>,
    /// Probes that fired without an interval field. A negative output start is never
    /// captured as a number, so it is counted here instead of reading as frame zero.
    pub unparsed: usize,
}

impl UnderrunLedger {
    #[must_use]
    pub fn from_probes(probes: &[ProbeEvent]) -> Self {
        let mut ledger = Self::default();
        for probe in probes
            .iter()
            .filter(|probe| probe.probe_name() == Some(UnderrunEvent::PROBE))
        {
            match UnderrunEvent::from_probe(probe) {
                Some(event) => ledger.events.push(event),
                None => ledger.unparsed += 1,
            }
        }
        ledger
    }

    /// Per-track totals, ordered by track id.
    #[must_use]
    pub fn tracks(&self) -> Vec<UnderrunTrack> {
        let mut tracks: Vec<UnderrunTrack> = Vec::new();
        for event in &self.events {
            let silence = event.silence();
            let known = tracks
                .iter()
                .position(|track| track.track_id == event.track_id);
            let index = known.unwrap_or_else(|| {
                tracks.push(UnderrunTrack {
                    track_id: event.track_id,
                    underruns: 0,
                    silenced_frames: 0,
                    first_output_frame: silence.start,
                    last_output_frame: silence.end,
                });
                tracks.len() - 1
            });
            let track = &mut tracks[index];
            track.underruns += 1;
            track.silenced_frames += silence.end - silence.start;
            track.first_output_frame = track.first_output_frame.min(silence.start);
            track.last_output_frame = track.last_output_frame.max(silence.end);
        }
        tracks.sort_by_key(|track| track.track_id);
        tracks
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kithara_test_utils::kithara;

    use super::*;

    fn probe(fields: &[(&'static str, u64)]) -> ProbeEvent {
        ProbeEvent {
            fields: fields.iter().copied().collect(),
            string_fields: HashMap::from([("probe", UnderrunEvent::PROBE.to_owned())]),
            at: ::kithara::platform::time::Instant::now(),
            target: "kithara_play_probe".to_owned(),
        }
    }

    #[kithara::test]
    fn a_starved_block_is_read_at_the_frames_it_silenced() {
        let ledger = UnderrunLedger::from_probes(&[
            probe(&[
                ("track_id", 3),
                ("output_start", 1_000),
                ("requested_frames", 128),
                ("available_frames", 28),
            ]),
            probe(&[
                ("track_id", 3),
                ("output_start", 1_128),
                ("requested_frames", 128),
                ("available_frames", 0),
            ]),
        ]);

        assert_eq!(ledger.unparsed, 0);
        assert_eq!(ledger.events[0].silence(), 1_028..1_128);
        assert_eq!(
            ledger.tracks(),
            [UnderrunTrack {
                track_id: Some(3),
                underruns: 2,
                silenced_frames: 228,
                first_output_frame: 1_028,
                last_output_frame: 1_256,
            }],
        );
    }

    #[kithara::test]
    fn a_probe_without_an_interval_is_counted_not_placed_at_frame_zero() {
        let ledger = UnderrunLedger::from_probes(&[probe(&[
            ("track_id", 3),
            ("requested_frames", 128),
            ("available_frames", 0),
        ])]);

        assert!(ledger.events.is_empty());
        assert_eq!(ledger.unparsed, 1);
    }

    #[kithara::test]
    fn probes_of_another_name_are_not_underruns() {
        let mut other = probe(&[
            ("track_id", 3),
            ("output_start", 64),
            ("requested_frames", 128),
            ("available_frames", 0),
        ]);
        other
            .string_fields
            .insert("probe", "pcm_consumed".to_owned());

        let ledger = UnderrunLedger::from_probes(&[other]);

        assert_eq!(ledger, UnderrunLedger::default());
    }

    #[kithara::test]
    fn a_block_that_delivered_every_frame_silences_nothing() {
        let ledger = UnderrunLedger::from_probes(&[probe(&[
            ("track_id", 3),
            ("output_start", 64),
            ("requested_frames", 128),
            ("available_frames", 128),
        ])]);

        assert!(ledger.events[0].silence().is_empty());
    }

    #[kithara::test]
    fn a_probe_without_a_track_stays_untracked() {
        let ledger = UnderrunLedger::from_probes(&[probe(&[
            ("output_start", 64),
            ("requested_frames", 128),
            ("available_frames", 64),
        ])]);

        assert_eq!(ledger.tracks()[0].track_id, None);
    }
}
