//! Commands executed at segment-relative output frames.

use std::{
    cmp::Ordering,
    convert::Infallible,
    num::{NonZeroU32, NonZeroUsize},
    task::{Context, Poll},
};

use kithara_audio::{AudioSource, TrackFailureKind};
use kithara_bufpool::HasPool;
use kithara_command::{Inbox, Protocol, Seq};
use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioSpec, FrameCount, SegmentId};
use kithara_warp::{SpeedCurve, StretchKind, WarpRenderer};
use num_traits::ToPrimitive;

/// Commands and receipts of one producer lane.
#[derive(Debug)]
pub enum LaneProtocol {}

/// An output frame within a lane segment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LaneFrame {
    pub segment: SegmentId,
    pub frame: u64,
}

/// A change executed at the lane's output cursor.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum LaneCommand {
    SetSpeed(SpeedCurve),
    Jump {
        to: Duration,
    },
    Segment {
        id: SegmentId,
        from: Duration,
        speed: SpeedCurve,
    },
    SetKeylock(bool),
    SetBackend(StretchKind),
    SetHostRate {
        id: SegmentId,
        rate: NonZeroU32,
    },
}

/// Latency of the resulting engine and the segment whose preload is admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaneApplied {
    pub engine_latency: FrameCount,
    pub ready: Option<SegmentId>,
}

impl Protocol for LaneProtocol {
    type Applied = LaneApplied;
    type Clock = LaneFrame;
    type Command = LaneCommand;
    type Refusal = Infallible;
    type Target = Infallible;

    fn frames_since(at: LaneFrame, start: LaneFrame) -> Option<u64> {
        match at.segment.cmp(&start.segment) {
            Ordering::Less => None,
            Ordering::Equal => at.frame.checked_sub(start.frame),
            Ordering::Greater => Some(u64::MAX),
        }
    }
}

#[derive(Clone, Copy)]
enum Jump {
    Down {
        to: Duration,
        start: u64,
        frames: usize,
    },
    Up {
        start: u64,
        frames: usize,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum LaneChange {
    None,
    Controls,
    Source,
}

pub(crate) struct Lane {
    inbox: Inbox<LaneProtocol>,
    cursor: LaneFrame,
    position: Option<Duration>,
    preload_chunks: NonZeroUsize,
    admitted: usize,
    pending: Option<(Seq, FrameCount)>,
    jump: Option<Jump>,
}

impl Lane {
    pub(crate) fn new(inbox: Inbox<LaneProtocol>, preload_chunks: NonZeroUsize) -> Self {
        Self {
            inbox,
            cursor: LaneFrame::default(),
            position: None,
            preload_chunks,
            admitted: 0,
            pending: None,
            jump: None,
        }
    }

    pub(crate) const fn cursor(&self) -> LaneFrame {
        self.cursor
    }

    pub(crate) const fn position(&self) -> Option<Duration> {
        self.position
    }

    pub(crate) fn poll_commands(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.inbox.poll_drain(context)
    }

    pub(crate) fn is_preloaded(&self) -> bool {
        self.admitted >= self.preload_chunks.get()
    }

    fn finish_pending(&mut self, ready: Option<SegmentId>) {
        if let Some((seq, latency)) = self.pending.take() {
            if let Some(due) = self.inbox.resume(seq, self.cursor, self.cursor) {
                due.apply(LaneApplied {
                    engine_latency: latency,
                    ready,
                });
            }
        }
    }

    pub(crate) fn admitted(&mut self) {
        self.admitted = self.admitted.saturating_add(1);
        if self.is_preloaded() {
            self.finish_pending(Some(self.cursor.segment));
        }
    }

    pub(crate) fn execute_due<T, S>(
        &mut self,
        source: &mut T,
        warp: &mut WarpRenderer<S>,
        spec: AudioSpec,
    ) -> Result<LaneChange, TrackFailureKind>
    where
        T: AudioSource<Chunk = AudioChunk>,
        S: HasPool<f32>,
    {
        self.inbox.drain();
        let mut changed = LaneChange::None;
        loop {
            let Some(due) = self.inbox.next_due(self.cursor, 1) else {
                break;
            };
            if changed == LaneChange::None {
                changed = LaneChange::Controls;
            }
            let revision = due.seq().get();
            let mut segment = None;
            for command in due.commands() {
                match command {
                    LaneCommand::SetSpeed(curve) => warp
                        .set_speed(curve.clone(), revision)
                        .map_err(|_| TrackFailureKind::Render)?,
                    LaneCommand::SetKeylock(on) => warp.set_keylock(*on),
                    LaneCommand::SetBackend(kind) => warp.set_backend(*kind),
                    LaneCommand::Jump { to } => {
                        let frames = (f64::from(spec.sample_rate.get())
                            * f64::from(crate::consts::DEFAULT_DECLICK.smooth_seconds))
                        .to_usize()
                        .map_or(1, |frames| frames.max(1));
                        self.jump = Some(Jump::Down {
                            to: *to,
                            start: self.cursor.frame,
                            frames,
                        });
                    }
                    LaneCommand::Segment { id, from, speed } => {
                        self.cursor = LaneFrame {
                            segment: *id,
                            frame: 0,
                        };
                        self.position = Some(landing_position(source.seek(*from).map_err(|error| TrackFailureKind::from(&error))?));
                        warp.reset();
                        warp.set_speed(speed.clone(), revision).map_err(|_| TrackFailureKind::Render)?;
                        self.jump = None;
                        segment = Some(*id);
                        changed = LaneChange::Source;
                    }
                    LaneCommand::SetHostRate { id, rate } => {
                        source.set_host_sample_rate(*rate);
                        warp.reset();
                        self.cursor = LaneFrame {
                            segment: *id,
                            frame: 0,
                        };
                        self.jump = None;
                        segment = Some(*id);
                        changed = LaneChange::Source;
                    }
                }
            }
            let next_spec = source.prepare_deferred().unwrap_or(spec);
            let latency = warp
                .prepare_engine_latency(next_spec)
                .map_err(|_| TrackFailureKind::Render)?;
            if segment.is_some() {
                let seq = due.defer();
                self.finish_pending(None);
                self.admitted = 0;
                self.pending = Some((seq, latency));
            } else {
                due.apply(LaneApplied {
                    engine_latency: latency,
                    ready: None,
                });
            }
        }
        if let Some(Jump::Down { to, start, frames }) = self.jump {
            let frames = u64::try_from(frames).map_or(u64::MAX, |frames| frames);
            if self.cursor.frame.saturating_sub(start) >= frames {
                self.position = Some(landing_position(source.seek(to).map_err(|error| TrackFailureKind::from(&error))?));
                warp.reset();
                let next_spec = source.prepare_deferred().unwrap_or(spec);
                warp.prepare_engine_latency(next_spec)
                    .map_err(|_| TrackFailureKind::Render)?;
                self.jump = Some(Jump::Up {
                    start: self.cursor.frame,
                    frames: usize::try_from(frames).map_or(usize::MAX, |frames| frames),
                });
                changed = LaneChange::Source;
            }
        }
        Ok(changed)
    }

    pub(crate) fn output_limit(&self) -> usize {
        let due = self
            .inbox
            .frames_until_due(self.cursor)
            .map_or(usize::MAX, |frames| {
                usize::try_from(frames).map_or(usize::MAX, |frames| frames)
            });
        let jump = match self.jump {
            Some(Jump::Down { start, frames, .. }) => frames.saturating_sub(
                usize::try_from(self.cursor.frame.saturating_sub(start))
                    .map_or(usize::MAX, |frames| frames),
            ),
            _ => usize::MAX,
        };
        due.min(jump)
    }

    pub(crate) fn stamp(&mut self, chunk: &mut AudioChunk) {
        chunk.meta.segment = self.cursor.segment;
        chunk.meta.lane_frame = self.cursor.frame;
        let channels = usize::from(chunk.spec().channels.max(1));
        if let Some(jump) = &self.jump {
            let (start, frames, down) = match *jump {
                Jump::Down { start, frames, .. } => (start, frames, true),
                Jump::Up { start, frames } => (start, frames, false),
            };
            for (offset, frame) in chunk.samples.chunks_exact_mut(channels).enumerate() {
                let offset = u64::try_from(offset).map_or(u64::MAX, |offset| offset);
                let elapsed = self
                    .cursor
                    .frame
                    .saturating_add(offset)
                    .saturating_sub(start);
                let progress = elapsed.to_f32().map_or(1.0, |elapsed| elapsed)
                    / frames.to_f32().map_or(1.0, |frames| frames);
                let gain = if down {
                    1.0 - progress.min(1.0)
                } else {
                    progress.min(1.0)
                };
                for sample in frame {
                    *sample *= gain;
                }
            }
        }
        let frames = u64::try_from(chunk.frames()).map_or(u64::MAX, |frames| frames);
        self.cursor.frame = self.cursor.frame.saturating_add(frames);
        if let Some(position) = chunk
            .meta
            .source_span
            .and_then(|span| span.position_at(span.output_frames()))
        {
            self.position = Some(position);
        }
        if matches!(self.jump, Some(Jump::Up { start, frames })
            if self.cursor.frame.saturating_sub(start) >= u64::try_from(frames).map_or(u64::MAX, |frames| frames))
        {
            self.jump = None;
        }
    }
}

fn landing_position(outcome: kithara_audio::SeekOutcome) -> Duration {
    match outcome {
        kithara_audio::SeekOutcome::Landed { landed_at, .. } => landed_at,
        kithara_audio::SeekOutcome::PastEof { duration, .. } => duration,
    }
}

#[cfg(test)]
mod tests {
    use kithara_audio::{AudioReadError, SeekOutcome, TrackStep, WaitingReason};
    use kithara_command::{Batch, ChannelConfig, Sender, When, channel};
    use kithara_dsp::param::SmootherConfig;
    use kithara_warp::{Warp, WarpConfig};
    use kithara_test_utils::kithara;

    use super::*;

    struct JumpSource;

    impl AudioSource for JumpSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            Ok(SeekOutcome::Landed { target, landed_at: target })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> { None }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            TrackStep::Blocked(WaitingReason::Waiting)
        }
    }

    fn jump(sender: &mut Sender<LaneProtocol>) {
        sender.send(When::Next, Batch {
            basis: Vec::new(),
            commands: vec![LaneCommand::Jump { to: Duration::from_secs(1) }],
        }).expect("jump credit");
    }

    #[kithara::test]
    #[case::default_declick(0.005, 221)]
    #[case::configured_declick(0.020, 882)]
    fn a_jump_fades_down_for_the_mixers_configured_declick(
        #[case] seconds: f32,
        #[case] expected: usize,
    ) {
        let spec = AudioSpec::new(1, NonZeroU32::new(44_100).expect("rate"));
        let config = crate::rt::DeckMixerConfig::builder().declick(SmootherConfig {
            smooth_seconds: seconds,
            ..SmootherConfig::default()
        }).build();
        assert_eq!(config.declick_frames(spec.sample_rate).get(), expected);
        let pools = crate::test_pools::pools();
        let (mut sender, inbox) = channel(ChannelConfig::builder().build());
        let mut lane = Lane::new(inbox, NonZeroUsize::new(1).expect("preload"));
        let mut warp = Warp::new((), &WarpConfig::builder().build()).renderer(spec, pools);
        jump(&mut sender);
        lane.execute_due(&mut JumpSource, &mut warp, spec).expect("jump accepted");
        assert_eq!(lane.output_limit(), expected, "Jump and mixer must share the configured, rounded declick length");
    }

    #[kithara::test]
    #[case::during_fade_down(false)]
    #[case::during_fade_up(true)]
    fn a_jump_during_another_jump_keeps_gain_continuous(#[case] during_up: bool) {
        let spec = AudioSpec::new(1, NonZeroU32::new(44_100).expect("rate"));
        let pools = crate::test_pools::pools();
        let (mut sender, inbox) = channel(ChannelConfig::builder().build());
        let mut lane = Lane::new(inbox, NonZeroUsize::new(1).expect("preload"));
        let mut warp = Warp::new((), &WarpConfig::builder().build()).renderer(spec, pools);
        jump(&mut sender);
        lane.execute_due(&mut JumpSource, &mut warp, spec).expect("first jump");
        if during_up {
            let frames = lane.output_limit();
            let mut down = crate::worker::packet_tests::chunk(spec, SegmentId::FIRST, 0, 0, &vec![1.0; frames]);
            lane.stamp(&mut down);
            lane.execute_due(&mut JumpSource, &mut warp, spec).expect("landing");
        }
        let mut before = crate::worker::packet_tests::chunk(spec, SegmentId::FIRST, lane.cursor().frame, 0, &[1.0; 100]);
        lane.stamp(&mut before);
        let previous = before.samples[99];
        jump(&mut sender);
        lane.execute_due(&mut JumpSource, &mut warp, spec).expect("second jump");
        let mut after = crate::worker::packet_tests::chunk(spec, SegmentId::FIRST, lane.cursor().frame, 0, &[1.0]);
        lane.stamp(&mut after);
        assert!((after.samples[0] - previous).abs() < 0.02,
            "retriggering Jump must not reset gain to unity: {previous} -> {}", after.samples[0]);
    }

    #[kithara::test]
    fn frames_since_orders_segments_before_offsets() {
        let start = LaneFrame {
            segment: SegmentId::FIRST.next(),
            frame: 8,
        };
        assert_eq!(
            LaneProtocol::frames_since(
                LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: u64::MAX
                },
                start
            ),
            None
        );
        assert_eq!(LaneProtocol::frames_since(start, start), Some(0));
        assert_eq!(
            LaneProtocol::frames_since(LaneFrame { frame: 7, ..start }, start),
            None
        );
        assert_eq!(
            LaneProtocol::frames_since(LaneFrame { frame: 9, ..start }, start),
            Some(1)
        );
        assert_eq!(
            LaneProtocol::frames_since(
                LaneFrame {
                    segment: start.segment.next(),
                    frame: 0
                },
                start
            ),
            Some(u64::MAX)
        );
    }
}
