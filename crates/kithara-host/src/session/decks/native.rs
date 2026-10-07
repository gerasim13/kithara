use std::task::{Wake, Waker};

use kithara_platform::sync::{Arc, Weak};
use kithara_play::PlayError;

use crate::{DeckId, HostOwner};

/// A wake of a held deck's postbox, never an ownership-transfer message.
pub(crate) enum DeckMsg {
    Drain(DeckId),
}

impl DeckMsg {
    pub(crate) fn run<S, O: HostOwner<S>>(self, owner: &mut O) {
        match self {
            Self::Drain(id) => {
                if let Err(error) =
                    owner.with_deck(id, &mut |deck, out, pass| deck.drain(pass, out))
                {
                    tracing::warn!(?id, %error, "host deck drain failed");
                }
            }
        }
    }
}

pub(crate) trait DeckInbox:
    kithara_platform::maybe_send::MaybeSend + kithara_platform::maybe_send::MaybeSync + 'static
{
    fn post(&self, message: DeckMsg) -> Result<(), PlayError>;
}

pub(crate) struct DeckWake {
    id: DeckId,
    inbox: Weak<dyn DeckInbox>,
}

impl DeckWake {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn waker(_inbox: &Arc<dyn DeckInbox>, _id: DeckId) -> Waker {
        todo!(
            "Create the owning browser thread wake for DeckMsg::Drain(id) without requiring a transferable HostedDeck (spec §5.3)"
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn waker(inbox: &Arc<dyn DeckInbox>, id: DeckId) -> Waker {
        Waker::from(Arc::new(Self {
            id,
            inbox: Arc::downgrade(inbox),
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Wake for DeckWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(inbox) = self.inbox.upgrade() {
            drop(inbox.post(DeckMsg::Drain(self.id)));
        }
    }
}
