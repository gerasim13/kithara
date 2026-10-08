//! One publication cut shared by a root executor and independently credited scopes.

use std::num::NonZeroU16;

use futures::task::AtomicWaker;
use kithara_config::Config;
use kithara_platform::sync::Arc;
use ringbuf::{HeapRb, traits::Split};

use super::{book::Book, docket::Docket, gate::Gate, sender::Sent};
use crate::{ChannelConfig, Protocol, Receipt, Seq};

mod inbox;
mod sender;
#[cfg(test)]
mod tests;

pub use self::{
    inbox::ScopedInbox,
    sender::{ScopeSender, ScopedSender},
};

/// A slot identity; retirement changes its generation before it can be reused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScopeId {
    index: u16,
    generation: u32,
}

impl ScopeId {
    /// The slot's fixed position in this channel.
    #[must_use]
    pub fn index(self) -> u16 {
        self.index
    }

    /// The slot's incarnation, incremented at retirement.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Capacities reserved once, before any real-time operation.
#[derive(Clone, Copy, Debug, Config)]
#[config(fields(value))]
#[non_exhaustive]
pub struct ScopedConfig {
    /// Root credits and target count.
    #[config(builder(default = ChannelConfig::builder().build()))]
    pub(crate) root: ChannelConfig,
    /// Independently reusable scope slots.
    #[config(builder(default = NonZeroU16::MIN))]
    pub(crate) scopes: NonZeroU16,
    /// Credits per scope and the maximum target count of an opened scope.
    #[config(builder(default = ChannelConfig::builder().build()))]
    pub(crate) scope: ChannelConfig,
}

/// An answer routed to its own level; Closed is the last answer of a generation.
#[derive(Debug)]
pub enum ScopedReceipt<R: Protocol, M: Protocol> {
    /// An ordinary root-level receipt.
    Root(Receipt<R>),
    /// An ordinary receipt tagged by the batch's own scope generation.
    Scope(ScopeId, Receipt<M>),
    /// All batches of this scope generation have been answered and it is free.
    Closed(ScopeId),
}

/// Why a scope cannot be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// Every scope slot is open or awaiting Closed.
    #[error("all scope slots are in use")]
    Exhausted,
    /// The requested target count exceeds the preallocated maximum.
    #[error("requested {targets} scope targets, limit is {limit}")]
    Targets {
        /// The requested count.
        targets: usize,
        /// The configured maximum.
        limit: usize,
    },
}

/// The identity does not name an open scope of this generation.
#[derive(Debug, thiserror::Error)]
#[error("the scope is not open in this generation")]
pub struct StaleScope;

/// The inbox has closed the publication gate.
#[derive(Debug, thiserror::Error)]
#[error("the channel's inbox is gone")]
pub struct Closed;

enum Item<R: Protocol, M: Protocol> {
    Root(Sent<R>),
    Scope { id: ScopeId, sent: Sent<M> },
    Close(ScopeId),
}

pub(super) enum ScopeReply<P: Protocol> {
    Receipt { id: ScopeId, receipt: Receipt<P> },
    Closed(ScopeId),
}

pub(super) struct Lifecycle {
    pub(super) generation: u32,
    pub(super) closing: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    Free,
    Open,
    Closing,
}

struct Slot<P: Protocol> {
    state: State,
    generation: u32,
    book: Book<P>,
}

struct ScopeDocket<P: Protocol> {
    docket: Docket<P>,
    lifecycle: Lifecycle,
}

/// Builds a root and its scope slots with independent credits and one publication ring.
#[must_use]
pub fn scoped_channel<R: Protocol, M: Protocol>(
    config: ScopedConfig,
) -> (ScopedSender<R, M>, ScopedInbox<R, M>) {
    let scopes = usize::from(config.scopes.get());
    let scope_capacity = scopes * (config.scope.capacity.get() + 1);
    let (commands, pending) =
        HeapRb::<Item<R, M>>::new(config.root.capacity.get() + scope_capacity).split();
    let (root_answers, root_receipts) =
        HeapRb::<Receipt<R>>::new(config.root.capacity.get()).split();
    let (scope_answers, scope_receipts) = HeapRb::<ScopeReply<M>>::new(scope_capacity).split();
    let gate = Arc::new(Gate::default());
    let answered = Arc::new(AtomicWaker::new());
    let sender = ScopedSender {
        commands: commands.freeze(),
        root_receipts,
        scope_receipts,
        root: Book::new(config.root.capacity.get(), config.root.targets),
        slots: (0..config.scopes.get())
            .map(|_| Slot {
                state: State::Free,
                generation: 0,
                book: Book::new(config.scope.capacity.get(), config.scope.targets),
            })
            .collect(),
        scope_targets: config.scope.targets,
        next: Seq::FIRST,
        gate: Arc::clone(&gate),
        answered: Arc::clone(&answered),
        holder: None,
    };
    let inbox = ScopedInbox {
        pending,
        root_answers,
        scope_answers,
        root: Docket::new(config.root),
        scopes: (0..config.scopes.get())
            .map(|_| ScopeDocket {
                docket: Docket::new(config.scope),
                lifecycle: Lifecycle {
                    generation: 0,
                    closing: false,
                },
            })
            .collect(),
        gate,
        answered,
    };
    (sender, inbox)
}
