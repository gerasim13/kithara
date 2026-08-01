use kithara_decode::{DecoderChunkOutcome, duration_for_frames, frames_for_duration};
use kithara_platform::time::Duration;
use kithara_stream::VariantTransition;
use tracing::{debug, trace};

use super::generation::DecoderGeneration;
use crate::pipeline::{
    rebuild::state::BuildId,
    seek::skip::{apply as apply_skip, apply_frames},
};

struct Consts;

impl Consts {
    const PRIME_STEPS_PER_PASS: usize = 8;
}

pub(crate) enum IncomingDecode {
    Preparing {
        transition: VariantTransition,
    },
    Building {
        transition: VariantTransition,
        build: BuildId,
    },
    Priming {
        transition: VariantTransition,
        generation: DecoderGeneration,
    },
    Failed {
        transition: VariantTransition,
        generation: DecoderGeneration,
    },
}

impl IncomingDecode {
    pub(crate) const fn transition(&self) -> VariantTransition {
        match self {
            Self::Preparing { transition }
            | Self::Building { transition, .. }
            | Self::Priming { transition, .. }
            | Self::Failed { transition, .. } => *transition,
        }
    }
}

impl From<IncomingDecode> for Option<DecoderGeneration> {
    fn from(incoming: IncomingDecode) -> Self {
        match incoming {
            IncomingDecode::Priming { generation, .. }
            | IncomingDecode::Failed { generation, .. } => Some(generation),
            IncomingDecode::Preparing { .. } | IncomingDecode::Building { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutgoingFrontier {
    Awaiting,
    Exact { frame: u64, rate: u32 },
    Unavailable,
}

#[derive(Clone, Copy)]
pub(crate) struct ReadyToPromote {
    overlap: OverlapSpan,
    transition: VariantTransition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct OverlapSpan {
    incoming_first: u64,
    incoming_next: u64,
}

impl ReadyToPromote {
    pub(crate) const fn transition(self) -> VariantTransition {
        self.transition
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IncomingPrime {
    Idle,
    Advanced,
    Pending,
    Ready,
    Failed,
}

impl super::core::ActiveDecode {
    pub(crate) fn incoming_transition(&self) -> Option<VariantTransition> {
        self.incoming.as_ref().map(IncomingDecode::transition)
    }

    /// Content time an incoming variant must be landed at to be spliceable.
    ///
    /// The audible playhead is not that time: it trails the decode frontier by
    /// the whole PCM ring, and a reader that pulls faster than real time can
    /// open the gap without bound. An incoming landed behind the frontier has
    /// to decode the entire gap before it can cover it, and it decodes no
    /// faster than the outgoing does — so the gap never closes and the switch
    /// never commits. Only [`OutgoingFrontier::Exact`] names a frame; the other
    /// two states carry no frontier to land against and the source keeps its
    /// own seek-derived target.
    ///
    /// The frontier itself, and never ahead of it. A promotion proof is minted
    /// when the outgoing walks *into* the staged span, and the outgoing stops
    /// decoding the moment its PCM ring is full — so a landing placed ahead of
    /// the frontier is a bet on motion the outgoing owes nobody, and a consumer
    /// that stops pulling wedges the switch for good. Landing on the frontier
    /// removes the bet: priming extends the staged *end*, and the end only has
    /// to overtake a frontier that moves no faster than the incoming decodes.
    pub(crate) fn landing_for(&self, outgoing_frontier: OutgoingFrontier) -> Option<Duration> {
        let OutgoingFrontier::Exact { frame, rate } = outgoing_frontier else {
            return None;
        };
        let origin = self.active.timeline_origin(self.gapless_mode());
        Some(duration_for_frames(rate, frame.saturating_sub(origin)))
    }

    pub(crate) fn begin_incoming(
        &mut self,
        transition: VariantTransition,
    ) -> Option<DecoderGeneration> {
        if self.incoming_transition() == Some(transition) {
            return None;
        }
        self.incoming
            .replace(IncomingDecode::Preparing { transition })
            .and_then(Into::into)
    }

    pub(crate) fn incoming_is_preparing(&self, transition: VariantTransition) -> bool {
        matches!(
            self.incoming,
            Some(IncomingDecode::Preparing {
                transition: current
            }) if current == transition
        )
    }

    #[cfg(test)]
    pub(crate) fn incoming_is_building(&self, transition: VariantTransition) -> bool {
        matches!(
            self.incoming,
            Some(IncomingDecode::Building {
                transition: current,
                ..
            }) if current == transition
        )
    }

    #[cfg(test)]
    pub(crate) fn incoming_is_priming(&self, transition: VariantTransition) -> bool {
        matches!(
            self.incoming,
            Some(IncomingDecode::Priming {
                transition: current,
                ..
            }) if current == transition
        )
    }

    pub(crate) fn mark_incoming_building(
        &mut self,
        transition: VariantTransition,
        build: BuildId,
    ) -> bool {
        if !self.incoming_is_preparing(transition) {
            return false;
        }
        self.incoming = Some(IncomingDecode::Building { transition, build });
        true
    }

    pub(crate) fn install_incoming(
        &mut self,
        transition: VariantTransition,
        build: BuildId,
        generation: DecoderGeneration,
    ) -> Option<DecoderGeneration> {
        if !matches!(
            self.incoming,
            Some(IncomingDecode::Building {
                transition: current,
                build: current_build,
            }) if current == transition && current_build == build
        ) {
            return Some(generation);
        }
        self.incoming = Some(IncomingDecode::Priming {
            transition,
            generation,
        });
        None
    }

    pub(crate) fn ready_to_promote(
        &self,
        outgoing_frontier: OutgoingFrontier,
    ) -> Option<ReadyToPromote> {
        let IncomingDecode::Priming {
            transition,
            generation,
        } = self.incoming.as_ref()?
        else {
            return None;
        };
        let gapless_mode = self.gapless_mode();
        let outgoing_origin = self.active.timeline_origin(gapless_mode);
        let incoming_origin = incoming_origin(&self.active, generation, gapless_mode);
        let overlap = overlap_span(
            generation,
            outgoing_frontier,
            outgoing_origin,
            incoming_origin,
        )?;
        Some(ReadyToPromote {
            overlap,
            transition: *transition,
        })
    }

    pub(crate) fn promote_incoming(&mut self, proof: ReadyToPromote) -> Option<DecoderGeneration> {
        let incoming = self.incoming.take()?;
        let IncomingDecode::Priming {
            transition,
            mut generation,
        } = incoming
        else {
            self.incoming = Some(incoming);
            return None;
        };
        if transition != proof.transition
            || !trim_staged_head(&mut generation, proof.overlap)
            || !generation.has_output()
        {
            self.incoming = Some(IncomingDecode::Priming {
                transition,
                generation,
            });
            return None;
        }
        self.blender.join_active(generation.blender_profile());
        Some(std::mem::replace(&mut self.active, generation))
    }

    pub(crate) fn discard_incoming(&mut self) -> Option<DecoderGeneration> {
        self.incoming.take().and_then(Into::into)
    }

    pub(crate) fn take_failed_incoming(
        &mut self,
    ) -> Option<(VariantTransition, DecoderGeneration)> {
        let incoming = self.incoming.take()?;
        match incoming {
            IncomingDecode::Failed {
                transition,
                generation,
            } => Some((transition, generation)),
            incoming => {
                self.incoming = Some(incoming);
                None
            }
        }
    }

    pub(crate) fn prime_incoming(&mut self, outgoing_frontier: OutgoingFrontier) -> IncomingPrime {
        let gapless_mode = self.gapless_mode();
        let outgoing_origin = self.active.timeline_origin(gapless_mode);
        let active_profile = self.active.gapless_profile();
        let active_gap = self.active.timeline_gap();
        let Some(IncomingDecode::Priming { generation, .. }) = self.incoming.as_mut() else {
            return IncomingPrime::Idle;
        };
        let incoming_origin =
            incoming_origin_from(active_profile, active_gap, generation, gapless_mode);
        if overlap_span(
            generation,
            outgoing_frontier,
            outgoing_origin,
            incoming_origin,
        )
        .is_some()
        {
            return IncomingPrime::Ready;
        }
        let mut outcome = IncomingPrime::Pending;
        for _ in 0..Consts::PRIME_STEPS_PER_PASS {
            outcome = match generation.next_chunk() {
                Ok(DecoderChunkOutcome::Chunk(chunk)) => {
                    let epoch = generation.installed_at_seek_epoch();
                    let Some(chunk) = apply_skip(chunk, epoch, generation.pending_head_skip_mut())
                    else {
                        continue;
                    };
                    if !chunk.samples.is_empty() {
                        generation.stage(chunk);
                    }
                    if overlap_span(
                        generation,
                        outgoing_frontier,
                        outgoing_origin,
                        incoming_origin,
                    )
                    .is_some()
                    {
                        IncomingPrime::Ready
                    } else {
                        outcome = IncomingPrime::Advanced;
                        continue;
                    }
                }
                Ok(DecoderChunkOutcome::Pending(_)) => IncomingPrime::Pending,
                Ok(DecoderChunkOutcome::Eof) => {
                    generation.finish_staging();
                    if overlap_span(
                        generation,
                        outgoing_frontier,
                        outgoing_origin,
                        incoming_origin,
                    )
                    .is_some()
                    {
                        IncomingPrime::Ready
                    } else if staged_ahead(
                        generation,
                        outgoing_frontier,
                        outgoing_origin,
                        incoming_origin,
                    ) {
                        IncomingPrime::Pending
                    } else {
                        debug!(
                            ?outgoing_frontier,
                            outgoing_origin,
                            "incoming generation reached EOF before the outgoing frontier"
                        );
                        IncomingPrime::Failed
                    }
                }
                Err(_) => IncomingPrime::Failed,
            };
            break;
        }
        if outcome == IncomingPrime::Failed
            && let Some(IncomingDecode::Priming {
                transition,
                generation,
            }) = self.incoming.take()
        {
            self.incoming = Some(IncomingDecode::Failed {
                transition,
                generation,
            });
        }
        outcome
    }

    pub(crate) fn flush_incoming_reader_signals(&mut self) {
        match self.incoming.as_mut() {
            Some(IncomingDecode::Priming { generation, .. })
            | Some(IncomingDecode::Failed { generation, .. }) => {
                generation.decoder_mut().flush_reader_signals();
            }
            Some(IncomingDecode::Preparing { .. } | IncomingDecode::Building { .. }) | None => {}
        }
    }
}

fn overlap_span(
    generation: &DecoderGeneration,
    outgoing_frontier: OutgoingFrontier,
    outgoing_origin: u64,
    incoming_origin: u64,
) -> Option<OverlapSpan> {
    let (incoming_first, incoming_end, incoming_rate) = generation.staged_span()?;
    let (outgoing_frame, outgoing_rate) = match outgoing_frontier {
        OutgoingFrontier::Awaiting => return None,
        OutgoingFrontier::Exact { frame, rate } => (frame, rate),
        OutgoingFrontier::Unavailable => {
            return Some(OverlapSpan {
                incoming_first,
                incoming_next: incoming_first,
            });
        }
    };
    let incoming_first_time = duration_for_frames(
        incoming_rate,
        incoming_first.saturating_sub(incoming_origin),
    );
    let incoming_end_time =
        duration_for_frames(incoming_rate, incoming_end.saturating_sub(incoming_origin));
    let outgoing_next = duration_for_frames(
        outgoing_rate,
        outgoing_frame.saturating_sub(outgoing_origin),
    );
    if incoming_first_time > outgoing_next || outgoing_next >= incoming_end_time {
        // Two very different situations share this exit. `outgoing_next >=
        // incoming_end_time` is ordinary: the incoming has not decoded far
        // enough yet and the next priming pass fixes it. `incoming_first_time >
        // outgoing_next` is not: priming only extends the staged END, so an
        // incoming that landed *after* the frame the outgoing is about to emit
        // can never cover it, and the transition waits for the outgoing to
        // catch up instead. Log the three quantities so the difference is
        // visible without a debugger.
        trace!(
            ?incoming_first_time,
            ?incoming_end_time,
            ?outgoing_next,
            incoming_first,
            incoming_origin,
            incoming_rate,
            outgoing_frame,
            outgoing_origin,
            outgoing_rate,
            landed_late = incoming_first_time > outgoing_next,
            "incoming generation does not cover the outgoing frontier"
        );
        return None;
    }
    let mut incoming_next =
        u64::try_from(frames_for_duration(incoming_rate, outgoing_next)).unwrap_or(u64::MAX);
    if duration_for_frames(incoming_rate, incoming_next) < outgoing_next {
        incoming_next = incoming_next.saturating_add(1);
    }
    incoming_next = incoming_next.saturating_add(incoming_origin);
    let span =
        (incoming_first <= incoming_next && incoming_next < incoming_end).then_some(OverlapSpan {
            incoming_first,
            incoming_next,
        });
    if span.is_some() {
        // Fires once per transition, and it is the one number the splice is
        // judged by: which incoming frame replaces the outgoing frontier. The
        // miss path already explains why a proof was refused; without this the
        // proof that succeeded says nothing about where it cut.
        debug!(
            incoming_first,
            incoming_next,
            incoming_end,
            incoming_origin,
            incoming_rate,
            outgoing_frame,
            outgoing_origin,
            outgoing_rate,
            ?outgoing_next,
            "incoming generation covers the outgoing frontier"
        );
    }
    span
}

fn staged_ahead(
    generation: &DecoderGeneration,
    outgoing_frontier: OutgoingFrontier,
    outgoing_origin: u64,
    incoming_origin: u64,
) -> bool {
    let Some((incoming_first, _, incoming_rate)) = generation.staged_span() else {
        return false;
    };
    match outgoing_frontier {
        OutgoingFrontier::Awaiting => true,
        OutgoingFrontier::Unavailable => false,
        OutgoingFrontier::Exact { frame, rate } => {
            duration_for_frames(
                incoming_rate,
                incoming_first.saturating_sub(incoming_origin),
            ) > duration_for_frames(rate, frame.saturating_sub(outgoing_origin))
        }
    }
}

fn trim_staged_head(generation: &mut DecoderGeneration, overlap: OverlapSpan) -> bool {
    let mut remaining = overlap.incoming_next.saturating_sub(overlap.incoming_first);
    if remaining == 0 {
        return generation.has_output();
    }
    while remaining != 0 {
        let Some(chunk) = generation.pop_staged() else {
            return false;
        };
        if let Some(chunk) = apply_frames(chunk, &mut remaining) {
            generation.push_staged_front(chunk);
        }
    }
    generation.has_output()
}

fn incoming_origin(
    active: &DecoderGeneration,
    incoming: &DecoderGeneration,
    mode: kithara_decode::GaplessMode,
) -> u64 {
    incoming_origin_from(
        active.gapless_profile(),
        active.timeline_gap(),
        incoming,
        mode,
    )
}

fn incoming_origin_from(
    active_profile: kithara_decode::GaplessProfile,
    active_gap: u64,
    incoming: &DecoderGeneration,
    mode: kithara_decode::GaplessMode,
) -> u64 {
    let incoming_profile = incoming.gapless_profile();
    let same_fallback_profile = active_profile.gapless().is_none()
        && incoming_profile.gapless().is_none()
        && active_profile.fallback_priming_frames() == incoming_profile.fallback_priming_frames();
    let gap = if same_fallback_profile {
        active_gap.max(incoming.timeline_gap())
    } else {
        incoming.timeline_gap()
    };
    incoming.timeline_origin_with_gap(mode, gap)
}
