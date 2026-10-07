use firewheel::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch, PatchError},
    event::ParamData,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        NodeError,
    },
};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::sync::{Arc, Mutex};

use super::{
    DeckMixerConfig,
    processor::{ContextRequirement, DeckMixer, StreamShape},
};
use crate::bridge::{MixerInputs, mixer_channels};

/// The audio node of one deck: its processor mixes the deck's slots.
///
/// The deck's owner drives it through the ring of [`mixer_channels`]; only
/// `active` participates in Firewheel parameter updates.
#[derive(Diff)]
#[derive_where::derive_where(Clone)]
pub struct PlayerNode<S> {
    /// Whether the node is active (used by Diff/Patch for graph updates).
    pub(crate) active: bool,

    /// Mixer ends taken by the first processor.
    #[diff(skip)]
    inputs: Arc<Mutex<Option<MixerInputs>>>,

    #[diff(skip)]
    context_requirement: ContextRequirement,

    /// Typed pool facade for scratch buffer allocation.
    #[diff(skip)]
    pools: PoolRegion<S>,

    #[diff(skip)]
    mixer: DeckMixerConfig,
}

/// A runtime parameter patch for [`PlayerNode`].
#[non_exhaustive]
pub enum PlayerNodePatch {
    /// Updates whether the node is active.
    Active(<bool as Patch>::Patch),
}

impl<S> Patch for PlayerNode<S> {
    type Patch = PlayerNodePatch;

    fn apply(&mut self, patch: Self::Patch) {
        match patch {
            PlayerNodePatch::Active(patch) => self.active.apply(patch),
        }
    }

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        match path {
            [0, tail @ ..] => Ok(PlayerNodePatch::Active(bool::patch(data, tail)?)),
            _ => Err(PatchError::InvalidPath),
        }
    }
}

impl<S> PlayerNode<S> {
    /// Create the node of the deck whose mixer ends are `inputs`.
    pub fn new(inputs: MixerInputs, pools: PoolRegion<S>) -> Self {
        Self {
            pools,
            mixer: inputs.config,
            active: true,
            inputs: Arc::new(Mutex::new(Some(inputs))),
            context_requirement: ContextRequirement::Standalone,
        }
    }

    /// Requires the Host-written render context when constructing this node's
    /// processor. Standalone nodes retain their context-free contract.
    #[doc(hidden)]
    #[must_use]
    pub fn with_session_context(mut self) -> Self {
        self.context_requirement = ContextRequirement::Session;
        self
    }
}

impl<S> AudioNode for PlayerNode<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let sample_rate = cx.stream_info.sample_rate;
        let max_block_frames = cx.stream_info.max_block_frames;
        let shape = StreamShape {
            max_block_frames,
            sample_rate,
        };
        let inputs = self
            .inputs
            .lock()
            .take()
            .unwrap_or_else(|| mixer_channels(self.mixer).1);
        Ok(DeckMixer::with_context_requirement(
            inputs,
            shape,
            &self.pools,
            self.context_requirement,
        ))
    }

    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("Player")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::STEREO,
            }))
    }
}

#[cfg(test)]
mod tests {
    use kithara_command::{Batch, When};
    use kithara_signal::SessionFrame;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        bridge::{DeckApplied, DeckEnds, DeckPart, Fade, Slot, SlotState},
        test_pools::{TestPools, pools},
    };

    fn make_node() -> (PlayerNode<TestPools>, DeckEnds) {
        let (ends, inputs) = mixer_channels(DeckMixerConfig::default());
        (PlayerNode::new(inputs, pools()), ends)
    }

    #[kithara::test]
    fn player_node_defaults_active() {
        let (node, _ends) = make_node();
        assert!(node.active);
    }

    #[kithara::test]
    fn player_node_info_has_stereo_output() {
        let (node, _ends) = make_node();
        let info = node.info(&EmptyConfig);
        let _ = info;
    }

    #[kithara::test]
    #[case(DeckPart::Start { slot: Slot::new(0), fade: Fade::Declick })]
    #[case(DeckPart::Stop { slot: Slot::new(0), fade: Fade::Declick })]
    #[case(DeckPart::Rate { slot: Slot::new(0), rate: 1.25 })]
    fn player_node_with_inputs(#[case] part: DeckPart) {
        let (node, mut ends) = make_node();
        assert!(node.active);

        ends.ring
            .send(
                When::Next,
                Batch {
                    basis: Vec::new(),
                    commands: vec![part],
                },
            )
            .expect("the deck channel has room");
        let received = node
            .inputs
            .lock()
            .as_mut()
            .map(|inputs| {
                inputs.inbox.drain();
                inputs.inbox.next_due(SessionFrame::default(), 1).map(|due| {
                    let parts = due.commands().len();
                    due.apply(DeckApplied::default());
                    parts
                })
            })
            .expect("inputs not yet taken");
        assert_eq!(received, Some(1));
    }

    #[kithara::test]
    fn player_node_snapshot_starts_with_empty_slots() {
        let (_node, mut ends) = make_node();
        let snapshot = ends.snapshot.read();
        assert_eq!(snapshot.slots.len(), DeckMixerConfig::default().slots().get());
        assert!(
            snapshot
                .slots
                .iter()
                .all(|slot| slot.state == SlotState::Empty)
        );
    }
}
