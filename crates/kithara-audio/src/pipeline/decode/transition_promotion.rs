use kithara_platform::time::Duration;
use kithara_stream::VariantTransition;

use super::transition::{
    IncomingDecode, OutgoingFrontier, PreparedPromotion, PromotionJoin, PromotionReadiness,
    incoming_origin, promotion_readiness, shares_default_profile, trim_staged_head,
};
use crate::pipeline::decode::{core::ActiveDecode, generation::DecoderGeneration};

impl ActiveDecode {
    pub(crate) fn commit_prepared_promotion(
        &mut self,
        mut prepared: PreparedPromotion,
    ) -> DecoderGeneration {
        if shares_default_profile(
            self.active.gapless_profile(),
            prepared.generation.gapless_profile(),
        ) {
            prepared
                .generation
                .align_timeline_gap(self.active.timeline_gap());
        }
        let incoming_profile = prepared.generation.blender_profile();
        if self.active.blender_profile().spec() != incoming_profile.spec()
            || !self.active.has_same_source_basis(&prepared.generation)
            || self.active.visible_origin() != prepared.generation.visible_origin()
            || self.active.media_origin() != prepared.generation.media_origin()
            || self.active.comparison_origin(self.gapless_mode())
                != incoming_origin(&self.active, &prepared.generation, self.gapless_mode())
        {
            self.discontinuity_revision = self.discontinuity_revision.wrapping_add(1);
        }
        if prepared.generation.visible_origin().is_none()
            && self.active.visible_origin().is_some()
            && let Some(origin) = self.landed_visible_origin(&prepared.generation)
        {
            let base = prepared.generation.comparison_base();
            prepared
                .generation
                .retain_visible_origin(Some(origin), base);
        }
        match prepared.join {
            PromotionJoin::HardCut => self.blender.replace_active(incoming_profile),
            PromotionJoin::Blend { .. } => self.blender.commit_join(),
        }
        std::mem::replace(&mut self.active, prepared.generation)
    }

    /// A landed incoming never observes its own start. Once the track's visible
    /// geometry is known, the incoming reports the visible origin of the raw start
    /// that the promotion's content alignment assumes for it.
    fn landed_visible_origin(&self, incoming: &DecoderGeneration) -> Option<Duration> {
        let mode = self.gapless_mode();
        let raw_start = incoming_origin(&self.active, incoming, mode)?
            .checked_sub(incoming.timeline_origin_with_gap(mode, 0))?;
        let profile = incoming.gapless_profile();
        let position = profile.spec().duration_for(raw_start).ok()?;
        crate::pipeline::gapless::visible_origin(position, profile, mode)
    }

    pub(crate) fn outgoing_holdback_needs_pcm(&self) -> bool {
        let Some(IncomingDecode::Priming {
            frontier,
            generation,
            ..
        }) = self.incoming.as_ref()
        else {
            return false;
        };
        let gapless_mode = self.gapless_mode();
        let Some((outgoing_origin, incoming_origin)) = self
            .active
            .comparison_origin(gapless_mode)
            .zip(incoming_origin(&self.active, generation, gapless_mode))
        else {
            return false;
        };
        matches!(
            promotion_readiness(
                &self.active,
                &self.blender,
                generation,
                *frontier,
                outgoing_origin,
                incoming_origin,
            ),
            PromotionReadiness::AwaitingOutgoingPcm
        )
    }

    /// A finished incoming may trim to an empty tail: the end-of-track hard cut proves nothing
    /// exists past the cut, and an empty tail is exactly consistent with that proof.
    pub(crate) fn prepare_promotion(&mut self) -> Option<PreparedPromotion> {
        let (transition, span, incoming_end) = {
            let IncomingDecode::Priming {
                transition,
                generation,
                frontier,
            } = self.incoming.as_ref()?
            else {
                return None;
            };
            let gapless_mode = self.gapless_mode();
            let outgoing_origin = self.active.comparison_origin(gapless_mode)?;
            let incoming_origin = incoming_origin(&self.active, generation, gapless_mode)?;
            let PromotionReadiness::Ready(span) = promotion_readiness(
                &self.active,
                &self.blender,
                generation,
                *frontier,
                outgoing_origin,
                incoming_origin,
            ) else {
                return None;
            };
            let last = generation.staged.back()?.meta;
            let incoming_end = last.source_span.map_or_else(
                || crate::SourceEnd::new(last.frame_range().end, last.spec.sample_rate),
                |span| {
                    crate::SourceEnd::new(span.end(), span.sample_rate())
                        .with_mapping_revision(span.mapping_revision())
                },
            );
            (*transition, span, incoming_end)
        };
        let IncomingDecode::Priming { mut generation, .. } = self.incoming.take()? else {
            return None;
        };
        let trimmed = match trim_staged_head(&mut generation, span.overlap) {
            Ok(trimmed) => trimmed,
            Err(error) => {
                tracing::warn!(?error, "incoming promotion mapping failed");
                self.incoming = Some(IncomingDecode::Failed {
                    transition,
                    generation,
                });
                return None;
            }
        };
        assert!(
            trimmed || generation.is_finished(),
            "BUG: a minted incoming promotion no longer covers its proven cut"
        );
        if let PromotionJoin::Blend {
            frames,
            outgoing_first,
            spec,
        } = span.join
        {
            let active = &self.active;
            let copied = self.blender.prepare_join(|outgoing| {
                active.copy_staged_frames(spec, outgoing_first, frames, outgoing)
            });
            assert!(
                copied,
                "BUG: a minted incoming promotion lost its proven outgoing PCM"
            );
        }
        let resume_head = generation.staged.front().map_or(incoming_end, |first| {
            first.meta.source_span.map_or_else(
                || crate::SourceEnd::new(first.meta.frame_offset, first.meta.spec.sample_rate),
                |span| {
                    crate::SourceEnd::new(span.start(), span.sample_rate())
                        .with_mapping_revision(span.mapping_revision())
                },
            )
        });
        Some(PreparedPromotion {
            generation,
            resume_head,
            transition,
            join: span.join,
        })
    }

    pub(crate) fn restore_prepared_promotion(&mut self, prepared: PreparedPromotion) {
        self.incoming = Some(IncomingDecode::Priming {
            transition: prepared.transition,
            generation: prepared.generation,
            frontier: OutgoingFrontier::Awaiting,
        });
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

    pub(crate) fn transition_holds_output(&self) -> bool {
        if self.active.is_finished() || !self.blender.is_steady() {
            return false;
        }
        let Some(IncomingDecode::Priming {
            frontier,
            generation,
            ..
        }) = self.incoming.as_ref()
        else {
            return false;
        };
        if *frontier == OutgoingFrontier::Awaiting {
            return false;
        }
        let gapless_mode = self.gapless_mode();
        let Some((outgoing_origin, incoming_origin)) = self
            .active
            .comparison_origin(gapless_mode)
            .zip(incoming_origin(&self.active, generation, gapless_mode))
        else {
            return false;
        };
        !matches!(
            promotion_readiness(
                &self.active,
                &self.blender,
                generation,
                *frontier,
                outgoing_origin,
                incoming_origin,
            ),
            PromotionReadiness::AwaitingOutgoingFrontier
        )
    }
}
