use kithara_platform::sync::Arc;
use kithara_render::bridge::{PlaybackShared, SlotControl};
use kithara_warp::RenderSnapshot;

use crate::api::SlotId;

/// The control half of the deck's slot once its Host has built it, under the id the session
/// built it with.
#[derive(Default)]
pub(super) struct DeckSlot {
    slot: Option<(SlotId, SlotControl)>,
}

impl DeckSlot {
    pub(super) fn get(&self, slot: SlotId) -> Option<&SlotControl> {
        self.slot
            .as_ref()
            .and_then(|(id, control)| (*id == slot).then_some(control))
    }

    pub(super) fn get_mut(&mut self, slot: SlotId) -> Option<&mut SlotControl> {
        self.slot
            .as_mut()
            .and_then(|(id, control)| (*id == slot).then_some(control))
    }

    pub(super) fn id(&self) -> Option<SlotId> {
        self.slot.as_ref().map(|(id, _)| *id)
    }

    pub(super) fn set(&mut self, slot: SlotId, control: SlotControl) {
        self.slot = Some((slot, control));
    }

    delegate::delegate! {
        to self {
            #[expr($.map(|control| Arc::clone(&control.playback)))]
            #[call(get)]
            pub(super) fn playback(&self, slot: SlotId) -> Option<Arc<PlaybackShared>>;
            #[expr($.and_then(SlotControl::latest_render_snapshot))]
            #[call(get)]
            pub(super) fn render_snapshot(&self, slot: SlotId) -> Option<RenderSnapshot>;
        }
    }
}
