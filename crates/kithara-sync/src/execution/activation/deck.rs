use std::{mem, num::NonZeroU32, ops::Range};

use kithara_warp::RenderContext;

use super::PreparedFirst;
use crate::LoadGeneration;

/// The kinds one deck's activation carries: the Player's item, the staged
/// lane, the resident track and the tail a switch fades out.
pub trait SyncKind: 'static {
    /// The Player's item a lane plays.
    type Item: Copy + Eq + Send + 'static;
    /// The installed lane a ticket carries.
    type Lane: Send + 'static;
    /// A resident track of the deck.
    type Track: Send + 'static;
    /// The previous reader of a switch, fading out.
    type Tail: Send + 'static;

    /// Whether `track` holds a lane, which leaves the callback through return
    /// custody and is never trashed.
    fn holds_lane(track: &Self::Track) -> bool;

    /// Whether `tail` has faded out, so it can be returned.
    fn settled(tail: &Self::Tail) -> bool;
}

/// One callback block's render surface for an activation.
pub trait ActivationDeck<K: SyncKind> {
    /// What one track render reports.
    type Outcome;
    /// The resident an activation is attempted on, held for the whole
    /// attempt.
    type Resident<'r>: ActivationResident<K, Outcome = Self::Outcome>
    where
        Self: 'r;

    /// The resident track of `item`, rendering in `context`.
    fn resident<'r>(
        &'r mut self,
        item: K::Item,
        context: &'r RenderContext,
    ) -> Option<Self::Resident<'r>>;

    /// Render the fading `tail` over `range` of the block in `context`.
    fn render_tail(&mut self, tail: &mut K::Tail, context: &RenderContext, range: Range<usize>);
}

/// The resident track of one activation attempt.
pub trait ActivationResident<K: SyncKind> {
    /// What one render of the resident reports.
    type Outcome;

    /// Whether the resident plays `load`, leads, and renders at
    /// `output_rate`.
    fn serves(&self, load: LoadGeneration, output_rate: NonZeroU32) -> bool;

    /// Render the resident's current reader over `range` of the block.
    fn render(&mut self, range: Range<usize>) -> Self::Outcome;

    /// Whether the resident still leads after rendering `prefix`, so the
    /// switch lands on continuous audio.
    fn leads_after(&self, prefix: Option<&Self::Outcome>) -> bool;

    /// Switch the resident to `lane` at block offset `at`: render its
    /// `first` frame in `first_context` and hand back the old reader.
    fn activate(
        &mut self,
        lane: K::Lane,
        first: &PreparedFirst,
        first_context: &RenderContext,
        at: usize,
    ) -> K::Tail;

    /// Render the fading `tail` over `range` of the block.
    fn render_tail(&mut self, tail: &mut K::Tail, range: Range<usize>);

    /// Render the new lane over `suffix` and report the whole block with the
    /// offset a following track takes over at.
    fn finish(&mut self, suffix: Range<usize>) -> (Self::Outcome, Option<usize>);
}

/// What an activation attempt leaves for the block's ordinary render.
pub enum SyncAttempt<I, O> {
    /// No ticket was attempted.
    None,
    /// The ticket was claimed: its resident rendered the whole block.
    Claimed {
        /// The item the claimed resident plays.
        item_id: I,
        /// The block the resident rendered.
        outcome: O,
        /// The offset a following track takes over at.
        handover_offset: Option<usize>,
    },
    /// The attempt stopped once the resident rendered the audio before the
    /// activation.
    PrefixRendered {
        /// The item the resident plays.
        item_id: I,
        /// The block offset the prefix ends at.
        offset: usize,
        /// The prefix render, when the activation was not at the block start.
        outcome: Option<O>,
    },
}

impl<I: Copy + Eq, O> SyncAttempt<I, O> {
    /// The claimed block of `item`, taken once.
    pub fn take_claimed(&mut self, item: I) -> Option<(O, Option<usize>)> {
        match mem::replace(self, Self::None) {
            Self::Claimed {
                item_id,
                outcome,
                handover_offset,
            } if item_id == item => Some((outcome, handover_offset)),
            other => {
                *self = other;
                None
            }
        }
    }
}
