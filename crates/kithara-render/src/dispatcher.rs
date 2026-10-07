//! The worker's dispatcher: the one place a track's source opens.

use std::{convert::Infallible, fmt::Debug, future::poll_fn, marker::PhantomData, task::Poll};

use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use kithara_command::{Inbox, Protocol};
use kithara_platform::maybe_send::MaybeSendFuture;

use crate::LoadRefusal;

/// What the dispatcher opens: a track's source, on the worker that will
/// render it.
pub trait Open: Debug {
    /// What an open hands to the owner that asked for it.
    type Opened: Debug;

    /// Opens the item. The worker holds a slot for it before the source
    /// opens, so a worker at capacity refuses without opening anything.
    fn open(self) -> impl MaybeSendFuture<Output = Result<Self::Opened, LoadRefusal>>;
}

/// The dispatcher's queue: each batch is one item to open, answered with what
/// it opened or why it did not.
#[derive(Debug)]
pub struct DispatcherProtocol<I>(PhantomData<fn() -> I>);

impl<I: Open> Protocol for DispatcherProtocol<I> {
    type Applied = I::Opened;
    type Clock = ();
    type Command = I;
    type Refusal = LoadRefusal;
    type Target = Infallible;

    fn frames_since((): (), (): ()) -> Option<u64> {
        Some(0)
    }
}

/// Opens the items its inbox receives, all at once, and answers each batch
/// with what its item opened or why it did not, in the order the opens end.
/// It ends when the sender is gone, dropping the opens still running.
///
/// A batch carries one item; a batch of any other size comes back
/// unanswered, whole.
pub async fn dispatch<I: Open>(mut inbox: Inbox<DispatcherProtocol<I>>) {
    let mut opening = FuturesUnordered::new();
    poll_fn(|cx| {
        loop {
            while let Poll::Ready(Some((seq, opened))) = opening.poll_next_unpin(cx) {
                let Some(due) = inbox.resume(seq, ()) else {
                    continue;
                };
                match opened {
                    Ok(opened) => due.apply(opened),
                    Err(refusal) => due.refuse(refusal),
                }
            }
            if inbox.poll_drain(cx).is_pending() {
                return Poll::Pending;
            }
            if inbox.is_closed() {
                return Poll::Ready(());
            }
            while let Some(mut due) = inbox.next_due((), 1) {
                if due.commands().len() != 1 {
                    continue;
                }
                let Some(item) = due.commands_mut().pop() else {
                    continue;
                };
                let seq = due.defer();
                opening.push(item.open().map(move |opened| (seq, opened)));
            }
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroUsize,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use futures::channel::oneshot;
    use kithara_command::{
        Batch, ChannelConfig, Outcome, Receipt, Rejection, Sender, When, channel,
    };
    use kithara_test_utils::kithara;

    use super::*;

    /// An item whose open ends with what the test sends it.
    #[derive(Debug)]
    struct Gate(oneshot::Receiver<Result<u32, LoadRefusal>>);

    impl Open for Gate {
        type Opened = u32;

        fn open(self) -> impl MaybeSendFuture<Output = Result<u32, LoadRefusal>> {
            async move {
                self.0
                    .await
                    .expect("the test answers every open it lets run")
            }
        }
    }

    type Protocol = DispatcherProtocol<Gate>;

    fn gate() -> (oneshot::Sender<Result<u32, LoadRefusal>>, Gate) {
        let (answer, opening) = oneshot::channel();
        (answer, Gate(opening))
    }

    fn pair() -> (Sender<Protocol>, Inbox<Protocol>) {
        channel(
            ChannelConfig::builder()
                .capacity(NonZeroUsize::new(4).expect("four batches"))
                .build(),
        )
    }

    fn send(sender: &mut Sender<Protocol>, items: Vec<Gate>) -> kithara_command::Seq {
        sender
            .send(
                When::Next,
                Batch {
                    basis: Vec::new(),
                    commands: items,
                },
            )
            .expect("the channel has room")
    }

    fn receipts(sender: &mut Sender<Protocol>) -> Vec<Receipt<Protocol>> {
        sender.receipts().collect()
    }

    #[kithara::test]
    fn opens_run_together_and_each_receipt_carries_its_own_open() {
        let (mut sender, inbox) = pair();
        let (first_answer, first_gate) = gate();
        let (second_answer, second_gate) = gate();
        let first = send(&mut sender, vec![first_gate]);
        let second = send(&mut sender, vec![second_gate]);
        let mut dispatcher = pin!(dispatch(inbox));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(dispatcher.as_mut().poll(&mut cx).is_pending());
        assert!(receipts(&mut sender).is_empty(), "both opens still run");

        second_answer.send(Ok(2)).expect("the second open waits");
        assert!(dispatcher.as_mut().poll(&mut cx).is_pending());
        let answered = receipts(&mut sender);
        assert!(
            matches!(
                answered.as_slice(),
                [receipt] if receipt.seq() == second
                    && matches!(receipt.outcome(), Outcome::Applied { data: 2, .. })
            ),
            "the second open ends first and answers its own batch: {answered:?}"
        );

        first_answer
            .send(Err(LoadRefusal::Capacity { capacity: 1 }))
            .expect("the first open still waits");
        assert!(dispatcher.as_mut().poll(&mut cx).is_pending());
        let answered = receipts(&mut sender);
        assert!(
            matches!(
                answered.as_slice(),
                [receipt] if receipt.seq() == first
                    && matches!(
                        receipt.outcome(),
                        Outcome::Rejected(Rejection::Refused(LoadRefusal::Capacity { capacity: 1 }))
                    )
            ),
            "a refused open answers its batch with the refusal: {answered:?}"
        );
    }

    #[kithara::test]
    fn a_batch_of_two_items_opens_neither() {
        let (mut sender, inbox) = pair();
        let (_first_answer, first_gate) = gate();
        let (_second_answer, second_gate) = gate();
        let both = send(&mut sender, vec![first_gate, second_gate]);
        let mut dispatcher = pin!(dispatch(inbox));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(dispatcher.as_mut().poll(&mut cx).is_pending());

        let answered = receipts(&mut sender);
        let [receipt] = answered.as_slice() else {
            panic!("the batch is answered at once: {answered:?}");
        };
        assert_eq!(receipt.seq(), both);
        assert!(matches!(
            receipt.outcome(),
            Outcome::Rejected(Rejection::Unanswered)
        ));
        let Some(receipt) = answered.into_iter().next() else {
            unreachable!("one receipt matched above");
        };
        let (_, returned): (Outcome<Protocol>, Batch<Protocol>) = receipt.into();
        assert_eq!(returned.commands.len(), 2, "both items come back unopened");
    }

    #[kithara::test]
    fn a_dropped_sender_ends_the_dispatcher_and_its_opens() {
        let (mut sender, inbox) = pair();
        let (answer, opening) = gate();
        send(&mut sender, vec![opening]);
        let mut dispatcher = pin!(dispatch(inbox));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(dispatcher.as_mut().poll(&mut cx).is_pending());
        assert!(!answer.is_canceled(), "the open runs");

        drop(sender);

        assert_eq!(dispatcher.as_mut().poll(&mut cx), Poll::Ready(()));
        assert!(answer.is_canceled(), "the dispatcher drops the open it ran");
    }
}
