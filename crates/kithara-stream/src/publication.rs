use std::sync::atomic::{AtomicU64, Ordering};

/// Identity of one reading session over a stream.
///
/// Minted by whoever owns the sessions of that stream, so ids are unique
/// within it and never reused — a session that lost the token can never be
/// mistaken for the one that took it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(u64);

impl From<u64> for SessionId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<SessionId> for u64 {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

/// Which reading session speaks for the stream.
///
/// A stream can have several readers but only one position — the byte a
/// consumer means when it asks where playback is. That is the frontier of the
/// session holding the token: what it has actually consumed, never what
/// anything has asked to be fetched. Sessions without the token read freely
/// and publish nothing, which is what lets one be built ahead of time without
/// dragging the position with it.
///
/// The projection is monotone within a seek epoch. A seek starts a new epoch
/// and rebases it; so does resolving an anchor the reader was handed before
/// the exact byte prefix was known, because the reader really is where the
/// correction says, and a projection that refused to follow would report
/// bytes nobody consumed.
pub struct Publication {
    holder: AtomicU64,
    frontier: AtomicU64,
    epoch: AtomicU64,
}

impl Publication {
    /// Open a publication whose token starts with `holder` — the session that
    /// is already reading when the stream opens.
    #[must_use]
    pub fn new(holder: SessionId) -> Self {
        Self {
            holder: AtomicU64::new(holder.0),
            frontier: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
        }
    }

    /// Consumed-to progress from `session`. Ignored unless it holds the token,
    /// and, within one seek epoch, unless it moves the projection forward.
    pub fn advance(&self, session: SessionId, frontier: u64, epoch: u64) {
        self.publish(session, frontier, epoch, false);
    }

    #[must_use]
    pub fn holds(&self, session: SessionId) -> bool {
        self.holder.load(Ordering::Acquire) == session.0
    }

    /// The published byte — what `Source::position` reports.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.frontier.load(Ordering::Acquire)
    }

    /// Repositioning from `session`: a seek, or an anchor resolved against
    /// exact byte sizes that were not known when the reader was handed it.
    /// Takes whatever the holder says, forward or back.
    pub fn rebase(&self, session: SessionId, frontier: u64, epoch: u64) {
        self.publish(session, frontier, epoch, true);
    }

    fn publish(&self, session: SessionId, frontier: u64, epoch: u64, rebase: bool) {
        if !self.holds(session) {
            return;
        }
        let published_epoch = self.epoch.load(Ordering::Acquire);
        if epoch < published_epoch {
            // A publication from before the seek the projection already
            // followed — the bytes it names belong to the old timeline.
            return;
        }
        if epoch > published_epoch {
            self.epoch.store(epoch, Ordering::Release);
            self.frontier.store(frontier, Ordering::Release);
            return;
        }
        if rebase {
            self.frontier.store(frontier, Ordering::Release);
        } else {
            self.frontier.fetch_max(frontier, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    /// The session that opened the stream, and one built alongside it.
    fn holder() -> SessionId {
        SessionId::from(1)
    }

    fn other() -> SessionId {
        SessionId::from(2)
    }

    fn opened() -> Publication {
        Publication::new(holder())
    }

    /// A session built alongside the audible one reads whatever it needs
    /// without moving what the stream reports.
    #[kithara::test]
    fn a_session_without_the_token_publishes_nothing() {
        let publication = opened();
        publication.advance(holder(), 100, 0);

        publication.advance(other(), 900, 0);
        publication.rebase(other(), 5, 0);

        assert_eq!(publication.position(), 100);
    }

    /// A session that is not the holder — because it never was, or because
    /// the token moved on — cannot drag the reported position anywhere.
    #[kithara::test]
    fn a_stale_holder_cannot_publish() {
        let publication = Publication::new(other());
        publication.advance(other(), 100, 0);

        publication.advance(holder(), 700, 0);
        publication.rebase(holder(), 5, 0);

        assert_eq!(publication.position(), 100);
    }

    /// Within one epoch the projection only goes forward: a stale read that
    /// lands late cannot drag the reported position backwards.
    #[kithara::test]
    fn progress_within_an_epoch_is_monotone() {
        let publication = opened();
        publication.advance(holder(), 100, 0);

        publication.advance(holder(), 40, 0);

        assert_eq!(publication.position(), 100);
    }

    /// A seek rebases it, backwards included — that is what a seek is.
    #[kithara::test]
    fn a_new_epoch_rebases_the_projection() {
        let publication = opened();
        publication.advance(holder(), 100, 0);

        publication.rebase(holder(), 40, 1);

        assert_eq!(publication.position(), 40);
    }

    /// An anchor resolved after the exact sizes arrived corrects the reader
    /// inside the same epoch, and the projection follows it back.
    #[kithara::test]
    fn a_rebase_within_the_epoch_follows_the_reader_back() {
        let publication = opened();
        publication.rebase(holder(), 100, 3);

        publication.rebase(holder(), 60, 3);

        assert_eq!(publication.position(), 60);
    }

    /// Publications from before the seek the projection already followed name
    /// bytes on the old timeline, so they are dropped rather than applied.
    #[kithara::test]
    fn an_old_epoch_publication_is_rejected() {
        let publication = opened();
        publication.rebase(holder(), 40, 1);

        publication.advance(holder(), 900, 0);
        publication.rebase(holder(), 900, 0);

        assert_eq!(publication.position(), 40);
    }
}
