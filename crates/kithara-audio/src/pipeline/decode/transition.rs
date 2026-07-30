use kithara_decode::DecoderChunkOutcome;
use kithara_stream::VariantTransition;

use super::generation::DecoderGeneration;
use crate::pipeline::rebuild::state::BuildId;

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

#[derive(Clone, Copy)]
pub(crate) struct ReadyToPromote {
    transition: VariantTransition,
}

impl ReadyToPromote {
    pub(crate) const fn transition(self) -> VariantTransition {
        self.transition
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IncomingPrime {
    Idle,
    Pending,
    Ready,
    Failed,
}

impl super::core::ActiveDecode {
    pub(crate) fn incoming_transition(&self) -> Option<VariantTransition> {
        self.incoming.as_ref().map(IncomingDecode::transition)
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

    pub(crate) fn ready_to_promote(&self) -> Option<ReadyToPromote> {
        let IncomingDecode::Priming {
            transition,
            generation,
        } = self.incoming.as_ref()?
        else {
            return None;
        };
        generation.has_output().then_some(ReadyToPromote {
            transition: *transition,
        })
    }

    pub(crate) fn promote_incoming(&mut self, proof: ReadyToPromote) -> Option<DecoderGeneration> {
        let incoming = self.incoming.take()?;
        let IncomingDecode::Priming {
            transition,
            generation,
        } = incoming
        else {
            self.incoming = Some(incoming);
            return None;
        };
        if transition != proof.transition || !generation.has_output() {
            self.incoming = Some(IncomingDecode::Priming {
                transition,
                generation,
            });
            return None;
        }
        self.blender.replace_active(generation.blender_profile());
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

    pub(crate) fn prime_incoming(&mut self) -> IncomingPrime {
        let Some(IncomingDecode::Priming { generation, .. }) = self.incoming.as_mut() else {
            return IncomingPrime::Idle;
        };
        if generation.has_output() {
            return IncomingPrime::Ready;
        }
        let outcome = match generation.next_chunk() {
            Ok(DecoderChunkOutcome::Chunk(chunk)) => {
                if !chunk.samples.is_empty() {
                    generation.push(chunk);
                }
                if generation.has_output() {
                    IncomingPrime::Ready
                } else {
                    IncomingPrime::Pending
                }
            }
            Ok(DecoderChunkOutcome::Pending(_)) => IncomingPrime::Pending,
            Ok(DecoderChunkOutcome::Eof) => {
                generation.finish();
                if generation.has_output() {
                    IncomingPrime::Ready
                } else {
                    IncomingPrime::Failed
                }
            }
            Err(_) => IncomingPrime::Failed,
        };
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
