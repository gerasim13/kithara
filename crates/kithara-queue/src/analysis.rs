use kithara_events::TrackId;
use kithara_platform::sync::Mutex;

/// Where the app leaves the producer half of an open analysis pass for a
/// track it expects to be loaded.
///
/// One slot, because one pass is open at a time: leaving a handle for another
/// track replaces the one waiting rather than queueing behind it. A handle is
/// taken by the load for its own track and by nothing else, so a track that
/// happens to load first cannot consume another's.
pub(crate) struct Mailbox<T>(Mutex<Option<(TrackId, T)>>);

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T> Mailbox<T> {
    /// Leave `handle` for `id`, dropping whatever was waiting.
    pub(crate) fn leave(&self, id: TrackId, handle: T) {
        *self.0.lock() = Some((id, handle));
    }

    /// Take the handle left for `id`, if that is the one waiting.
    pub(crate) fn take(&self, id: TrackId) -> Option<T> {
        let mut slot = self.0.lock();
        if slot.as_ref().is_some_and(|(waiting, _)| *waiting == id) {
            return slot.take().map(|(_, handle)| handle);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use kithara_events::TrackId;
    use kithara_test_utils::kithara;

    use super::Mailbox;

    #[kithara::test]
    fn a_handle_is_taken_only_by_the_track_it_was_left_for() {
        let mailbox = Mailbox::default();
        let (left, other) = (TrackId::from(1), TrackId::from(2));

        mailbox.leave(left, "pass");
        assert!(
            mailbox.take(other).is_none(),
            "another track's load leaves it alone"
        );
        assert_eq!(mailbox.take(left), Some("pass"), "its own load takes it");
        assert!(
            mailbox.take(left).is_none(),
            "and it is gone once it has been taken"
        );
    }

    #[kithara::test]
    fn a_newer_handle_replaces_the_one_waiting() {
        let mailbox = Mailbox::default();
        let (stale, current) = (TrackId::from(1), TrackId::from(2));

        mailbox.leave(stale, "first");
        mailbox.leave(current, "second");
        assert!(
            mailbox.take(stale).is_none(),
            "the pass it belonged to is no longer the open one"
        );
        assert_eq!(mailbox.take(current), Some("second"));
    }
}
