use std::marker::PhantomData;

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

use super::processor::{DeckMixer, StreamShape};
use crate::bridge::{MixerInputs, SessionInbox};

/// A deck node whose processor borrows its commands from the Host's session store.
#[derive(Diff)]
#[derive_where::derive_where(Clone)]
pub struct PlayerNode<S, E: SessionInbox> {
    pub(crate) active: bool,
    #[diff(skip)]
    inputs: Arc<Mutex<Option<MixerInputs>>>,
    #[diff(skip)]
    pools: PoolRegion<S>,
    #[diff(skip)]
    session: PhantomData<fn() -> E>,
}

#[non_exhaustive]
pub enum PlayerNodePatch {
    Active(<bool as Patch>::Patch),
}

impl<S, E: SessionInbox> Patch for PlayerNode<S, E> {
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

impl<S, E: SessionInbox> PlayerNode<S, E> {
    pub fn new(inputs: MixerInputs, pools: PoolRegion<S>) -> Self {
        Self {
            pools,
            active: true,
            inputs: Arc::new(Mutex::new(Some(inputs))),
            session: PhantomData,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("deck processor inputs have already been taken")]
struct ProcessorInputsTaken;

impl<S, E: SessionInbox> AudioNode for PlayerNode<S, E>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let shape = StreamShape::new(cx.stream_info.max_block_frames, cx.stream_info.sample_rate);
        let inputs = self
            .inputs
            .lock()
            .take()
            .ok_or_else(|| NodeError(Box::new(ProcessorInputsTaken)))?;
        DeckMixer::<E>::new(inputs, shape, &self.pools).map_err(|error| NodeError(Box::new(error)))
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
