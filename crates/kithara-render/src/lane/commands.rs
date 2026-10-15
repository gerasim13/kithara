use std::task::{Context, Poll};

use kithara_audio::{AudioSource, TrackFailureKind};
use kithara_bufpool::HasPool;
use kithara_signal::{AudioChunk, AudioSpec};
use kithara_warp::WarpRenderer;

use super::{
    Jump, Lane, LaneApplied, LaneChange, LaneCommand, LaneFrame, Preload, Preparation,
    landing_position,
};

impl Lane {
    pub(crate) fn poll_commands(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.inbox.poll_drain(context)
    }

    fn drain_commands(&mut self) -> Result<(), TrackFailureKind> {
        self.inbox.drain();
        loop {
            let next = match self.inbox.next_deferred() {
                Some(deferred) => deferred.park(),
                None => break,
            };
            let previous = self.deferred.replace(next.ok_or(TrackFailureKind::Render)?);
            if let Some(previous) = previous {
                drop(self.inbox.resume(previous, self.cursor, self.cursor));
            }
        }
        Ok(())
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
        self.drain_commands()?;
        let mut changed = LaneChange::None;
        let declick_frames = self.declick_frames(spec.sample_rate).get();
        loop {
            let deferred = self.ended
                && self
                    .inbox
                    .frames_until_due(self.cursor)
                    .is_none_or(|frames| frames > 0);
            let Some(due) = (if deferred && self.deferred.is_some() {
                let seq = self.deferred.take().ok_or(TrackFailureKind::Render)?;
                self.inbox.resume(seq, self.cursor, self.cursor)
            } else {
                self.inbox.next_due(self.cursor, 1)
            }) else {
                break;
            };
            if changed == LaneChange::None {
                changed = LaneChange::Controls;
            }
            let revision = due.seq().get();
            let at = due.at();
            let mut segment = None;
            let mut requested_entry = None;
            let mut restored_same = false;
            let mut cancelled_deferred = None;
            let mut refusal = None;
            for command in due.commands() {
                tracing::debug!(
                    seq = revision,
                    actual_apply_segment = at.segment.get(),
                    actual_apply_frame = at.frame,
                    current_segment = self.cursor.segment.get(),
                    prepared_frontier = self.cursor.frame,
                    command = ?command,
                    "lane command executing"
                );
                match command {
                    LaneCommand::SetSpeed(curve) => warp
                        .set_speed(curve.clone(), revision)
                        .map_err(|_| TrackFailureKind::Render)?,
                    LaneCommand::SetKeylock(on) => warp.set_keylock(*on),
                    LaneCommand::SetBackend(kind) => warp.set_backend(*kind),
                    LaneCommand::Jump { to } => {
                        let gain = self.jump.map_or(1.0, |jump| jump.gain(self.cursor.frame));
                        self.jump = Some(Jump::Down {
                            to: *to,
                            start: self.cursor.frame,
                            frames: declick_frames,
                            gain,
                        });
                    }
                    LaneCommand::Segment { id, from, speed } => {
                        if *id == self.cursor.segment && self.deferred.is_some() {
                            restored_same = true;
                            cancelled_deferred = self.deferred.take();
                            continue;
                        }
                        let restored = self.parked.filter(|(cursor, _)| cursor.segment == *id);
                        let from = match restored {
                            Some((_, Some(position))) => position,
                            Some((_, None)) => {
                                refusal = Some(TrackFailureKind::Render);
                                break;
                            }
                            None => *from,
                        };
                        if deferred {
                            self.parked = Some((self.cursor, self.position));
                        } else {
                            self.parked = None;
                        }
                        cancelled_deferred = self.deferred.take();
                        self.cursor = restored.map_or(
                            LaneFrame {
                                segment: *id,
                                frame: 0,
                            },
                            |(cursor, _)| cursor,
                        );
                        requested_entry = Some(from);
                        self.position = landing_position(source.seek_for_render(from))?;
                        warp.reset();
                        warp.set_speed(speed.clone(), revision)
                            .map_err(|_| TrackFailureKind::Render)?;
                        self.jump = None;
                        self.ended = false;
                        segment = Some(*id);
                        changed = LaneChange::Source;
                    }
                    LaneCommand::SetHostRate { id, rate } => {
                        cancelled_deferred = self.deferred.take();
                        self.parked = None;
                        source.set_host_sample_rate(*rate);
                        warp.reset();
                        self.cursor = LaneFrame {
                            segment: *id,
                            frame: 0,
                        };
                        self.jump = None;
                        self.ended = false;
                        segment = Some(*id);
                        changed = LaneChange::Source;
                    }
                }
            }
            if let Some(failure) = refusal {
                due.refuse(failure);
                return Err(failure);
            }
            let next_spec = source.prepare_deferred().unwrap_or(spec);
            let latency = warp
                .prepare_engine_latency(next_spec)
                .map_err(|_| TrackFailureKind::Render)?;
            if segment.is_some() {
                let seq = due.defer();
                self.finish_pending(None);
                self.preload = Preload::Filling(0);
                self.pending = Some(Preparation {
                    seq,
                    at,
                    entry: self.cursor,
                    source: None,
                    requested_entry,
                    render_revision: warp.render_revision(),
                    latency,
                });
            } else {
                if matches!(self.preload, Preload::Filling(0))
                    && let Some(pending) = &mut self.pending
                    && pending.entry == self.cursor
                {
                    pending.render_revision = warp.render_revision();
                    pending.latency = latency;
                }
                due.apply(LaneApplied {
                    source_basis: None,
                    engine_latency: latency,
                    ready: (restored_same && matches!(self.preload, Preload::Ready))
                        .then_some(self.cursor.segment),
                });
            }
            if let Some(seq) = cancelled_deferred {
                drop(self.inbox.resume(seq, self.cursor, self.cursor));
            }
        }
        if self.land_jump(source, warp, spec)? {
            changed = LaneChange::Source;
        }
        Ok(changed)
    }
}
