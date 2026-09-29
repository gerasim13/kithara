use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

use super::{ReturnedTicket, SyncKind, SyncTicket};
use crate::{SyncExecutionReject, consts, execution::arbiter::PermitState};

/// The ticket one deck of kind `K` receives.
type Ticket<K> = SyncTicket<<K as SyncKind>::Item, <K as SyncKind>::Lane>;

/// What of an unclaimed ticket of kind `K` comes back for release.
type Returned<K> = ReturnedTicket<<K as SyncKind>::Item, <K as SyncKind>::Lane>;

/// An audio-owned object handed back to the control thread, so the callback
/// frees nothing.
pub enum SyncReturn<K: SyncKind> {
    /// A ticket the callback rejected or found withdrawn.
    Ticket(Returned<K>),
    /// A lane-holding track the callback removed.
    Track(K::Track),
    /// The previous reader of a switch, once faded out.
    Tail(K::Tail),
}

/// The control half of one deck's activation rings.
pub struct ActivationControl<K: SyncKind> {
    tickets: HeapProd<Ticket<K>>,
    returns: HeapCons<SyncReturn<K>>,
    /// The ticket handed while the deck still held an earlier one; it
    /// enters the ring once the deck lets that one go.
    waiting: Option<Ticket<K>>,
}

/// The audio half of one deck's activation rings, taken by its callback.
pub struct ActivationAudio<K: SyncKind> {
    pub(super) pending: HeapCons<Ticket<K>>,
    pub(super) returns: HeapProd<SyncReturn<K>>,
}

/// Make one deck's ticket and return rings off the audio thread.
#[must_use]
pub fn activation_channels<K: SyncKind>() -> (ActivationControl<K>, ActivationAudio<K>) {
    let (tickets, pending) = HeapRb::new(consts::TICKET_RING).split();
    let (returns_tx, returns_rx) = HeapRb::new(consts::RETURN_RING).split();
    (
        ActivationControl {
            tickets,
            returns: returns_rx,
            waiting: None,
        },
        ActivationAudio {
            pending,
            returns: returns_tx,
        },
    )
}

impl<K: SyncKind> ActivationControl<K> {
    /// Hand `ticket` to the deck. While the deck still holds an earlier
    /// ticket, `ticket` waits and enters once the deck lets that one go. A
    /// waiting ticket the owner withdrew gives way to `ticket` and comes back
    /// for release.
    ///
    /// # Errors
    ///
    /// Returns `Capacity` while a ticket the owner has not withdrawn waits
    /// already; `ticket` is dropped.
    pub fn hand(&mut self, ticket: Ticket<K>) -> Result<Option<Returned<K>>, SyncExecutionReject> {
        self.forward();
        if self.waiting.as_ref().is_some_and(|waiting| {
            waiting.gate().permit_state(&waiting.permit()) != PermitState::Withdrawn
        }) {
            return Err(SyncExecutionReject::Capacity);
        }
        let displaced = self.waiting.replace(ticket).map(ReturnedTicket::from);
        self.forward();
        Ok(displaced)
    }

    /// The next object the deck handed back, once the waiting ticket has
    /// entered any room the deck made.
    pub fn next_return(&mut self) -> Option<SyncReturn<K>> {
        self.forward();
        self.returns.try_pop()
    }

    fn forward(&mut self) {
        if self.tickets.vacant_len() == 0 {
            return;
        }
        if let Some(ticket) = self.waiting.take() {
            let sent = self.tickets.try_push(ticket);
            debug_assert!(
                sent.is_ok(),
                "the ring had vacancy, and only the deck frees more"
            );
        }
    }
}

/// The return ring of one deck and the one return it could not take yet.
pub(super) struct ReturnCustody<K: SyncKind> {
    ring: HeapProd<SyncReturn<K>>,
    held: Option<SyncReturn<K>>,
}

impl<K: SyncKind> ReturnCustody<K> {
    pub(super) const fn new(ring: HeapProd<SyncReturn<K>>) -> Self {
        Self { ring, held: None }
    }

    /// Whether one more return fits, in the ring or in the held slot.
    pub(super) fn can_return(&self) -> bool {
        self.ring.vacant_len() > 0 || self.held.is_none()
    }

    /// Room for one return, in the ring or in the held slot.
    pub(super) fn room(&mut self) -> Option<ReturnRoom<'_, K>> {
        if self.can_return() {
            Some(ReturnRoom { custody: self })
        } else {
            None
        }
    }

    /// Room for one return in the ring itself.
    pub(super) fn ring_room(&mut self) -> Option<ReturnRoom<'_, K>> {
        if self.ring.vacant_len() > 0 {
            Some(ReturnRoom { custody: self })
        } else {
            None
        }
    }

    pub(super) const fn held_is_empty(&self) -> bool {
        self.held.is_none()
    }

    /// Move the held return into the ring once the ring has room.
    pub(super) fn flush_held(&mut self) {
        if let Some(returned) = self.held.take()
            && let Err(returned) = self.ring.try_push(returned)
        {
            self.held = Some(returned);
        }
    }
}

/// Proven room for one return, held exclusively until it is used.
#[must_use]
pub struct ReturnRoom<'a, K: SyncKind> {
    custody: &'a mut ReturnCustody<K>,
}

impl<K: SyncKind> ReturnRoom<'_, K> {
    /// Return a lane-holding `track` to the control thread.
    pub fn track(self, track: K::Track) {
        self.put(SyncReturn::Track(track));
    }

    /// Return `returned` into the ring, or into the held slot this room was
    /// issued for while the ring was full.
    pub(super) fn put(self, returned: SyncReturn<K>) {
        if let Err(returned) = self.custody.ring.try_push(returned) {
            debug_assert!(
                self.custody.held.is_none(),
                "a return room is issued only with ring vacancy or an empty held slot"
            );
            self.custody.held = Some(returned);
        }
    }
}

/// How a resident removed from the callback leaves it.
pub enum TrackDisposal<'a, K: SyncKind> {
    /// It holds no lane: the deck's trash takes it.
    Trash,
    /// It holds a lane: it returns through custody.
    Return(ReturnRoom<'a, K>),
}
