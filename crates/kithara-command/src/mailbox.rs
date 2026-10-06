use std::{mem, task::Waker};

use kithara_platform::sync::{Arc, Mutex};

/// The owner dropped its mailbox, so a post reaches no one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the owner dropped its mailbox")]
pub struct PostError;

/// Who runs the posts a mailbox takes.
enum Holder {
    Nobody,
    Executor(Waker),
    Gone,
}

struct State<C> {
    holder: Holder,
    posts: Vec<C>,
}

/// Creates a mailbox for an owner and the first postbox that reaches it.
#[must_use]
pub fn mailbox<C>() -> (Postbox<C>, Mailbox<C>) {
    let state = Arc::new(Mutex::new(State {
        holder: Holder::Nobody,
        posts: Vec::new(),
    }));
    (
        Postbox {
            state: Arc::clone(&state),
        },
        Mailbox { state },
    )
}

/// Posting half of a mailbox. Every clone posts into the same queue, so the
/// owner runs posts from all of them in the order they were posted.
pub struct Postbox<C> {
    state: Arc<Mutex<State<C>>>,
}

impl<C> Clone for Postbox<C> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<C> Postbox<C> {
    /// Queues `command` for the owner and wakes the executor that holds it.
    /// A post made while no executor holds the owner waits for the next one.
    ///
    /// # Errors
    ///
    /// Returns [`PostError`] once the owner dropped its mailbox; the command
    /// is dropped.
    pub fn post(&self, command: C) -> Result<(), PostError> {
        let waker = {
            let mut state = self.state.lock();
            let waker = match &state.holder {
                Holder::Gone => return Err(PostError),
                Holder::Nobody => None,
                Holder::Executor(waker) => Some(waker.clone()),
            };
            state.posts.push(command);
            waker
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}

/// Receiving half of a mailbox, kept by the owner its commands are for.
pub struct Mailbox<C> {
    state: Arc<Mutex<State<C>>>,
}

impl<C> Mailbox<C> {
    /// Wakes `waker` after each post from now on, so the executor behind it
    /// drains the owner. Posts that waited for a holder are the executor's
    /// to drain once it holds the owner.
    pub fn hold(&mut self, waker: Waker) {
        self.state.lock().holder = Holder::Executor(waker);
    }

    /// Stops waking the executor; posts wait for the next holder.
    pub fn release(&mut self) {
        self.state.lock().holder = Holder::Nobody;
    }

    /// Takes every post queued so far, in the order they were posted.
    pub fn drain(&mut self) -> impl Iterator<Item = C> + use<C> {
        mem::take(&mut self.state.lock().posts).into_iter()
    }
}

/// Drops the posts not drained yet, so a caller waiting on one of them stops
/// waiting, and refuses every later post.
impl<C> Drop for Mailbox<C> {
    fn drop(&mut self) {
        let posts = {
            let mut state = self.state.lock();
            state.holder = Holder::Gone;
            mem::take(&mut state.posts)
        };
        drop(posts);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        task::{Wake, Waker},
    };

    use kithara_platform::sync::{Arc, mpsc};
    use kithara_test_utils::kithara;

    use super::{PostError, mailbox};

    /// A holder's waker that counts its wakes.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn waker() -> (Waker, Arc<Wakes>) {
        let wakes = Arc::new(Wakes::default());
        (Waker::from(Arc::clone(&wakes)), wakes)
    }

    #[kithara::test]
    fn a_post_made_before_a_holder_waits_for_it() {
        let (postbox, mut mailbox) = mailbox::<u32>();
        let (waker, wakes) = waker();

        postbox.post(1).expect("an open mailbox takes the post");
        mailbox.hold(waker);

        assert_eq!(wakes.0.load(Ordering::Relaxed), 0, "nobody held it to wake");
        assert_eq!(mailbox.drain().collect::<Vec<_>>(), [1]);
    }

    #[kithara::test]
    fn a_post_wakes_the_holder() {
        let (postbox, mut mailbox) = mailbox::<u32>();
        let (waker, wakes) = waker();
        mailbox.hold(waker);

        postbox.post(1).expect("a held mailbox takes the post");

        assert_eq!(wakes.0.load(Ordering::Relaxed), 1, "one post, one wake");
    }

    #[kithara::test]
    fn the_holder_drains_posts_from_every_postbox_in_the_order_they_were_posted() {
        let (first, mut mailbox) = mailbox::<u32>();
        let second = first.clone();
        mailbox.hold(waker().0);

        for (postbox, command) in [(&first, 1), (&second, 2), (&first, 3), (&second, 4)] {
            postbox
                .post(command)
                .expect("a held mailbox takes the post");
        }

        assert_eq!(mailbox.drain().collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(mailbox.drain().count(), 0, "a drained post runs once");
    }

    #[kithara::test]
    fn a_post_after_release_wakes_no_one_and_waits_for_the_next_holder() {
        let (postbox, mut mailbox) = mailbox::<u32>();
        let (released, released_wakes) = waker();
        mailbox.hold(released);

        mailbox.release();
        postbox.post(1).expect("an open mailbox takes the post");

        assert_eq!(released_wakes.0.load(Ordering::Relaxed), 0);
        mailbox.hold(waker().0);
        assert_eq!(mailbox.drain().collect::<Vec<_>>(), [1]);
    }

    #[kithara::test]
    fn a_dropped_mailbox_drops_its_posts_and_closes_its_postboxes() {
        let (postbox, mailbox) = mailbox::<mpsc::Sender<u32>>();
        let (answer, reply) = mpsc::channel();
        postbox
            .post(answer)
            .expect("an open mailbox takes the post");

        drop(mailbox);

        assert!(
            reply.recv().is_err(),
            "a dropped post leaves its caller unanswered"
        );
        assert_eq!(postbox.post(mpsc::channel().0).err(), Some(PostError));
    }
}
