//! Dispatcher-thread ownership of open sources and resident render lanes.

use std::{
    convert::Infallible,
    fmt::{self, Debug},
    future::poll_fn,
    marker::PhantomData,
    num::NonZeroUsize,
    task::{Context, Poll, Waker},
};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use kithara_command::{Inbox, Protocol, Seq};
use kithara_platform::{maybe_send::MaybeSendFuture, time::Duration};
use kithara_signal::FrameCount;
use kithara_warp::{SpeedCurve, StretchKind};
use kithara_worker::{Priority, Task, TickResult};

use crate::{LaneProtocol, LoadRefusal, ServiceClass, worker::scheduler::Wake};

/// Dispatcher-issued identity in load admission order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LaneId(u64);

#[derive(Debug)]
pub enum DispatcherCommand<I> {
    Load(LoadRequest<I>),
    Release(LaneId),
    SetPriority(LaneId, ServiceClass),
}

pub struct LoadRequest<I> {
    pub item: I,
    pub position: Duration,
    pub start: LaneStart,
    pub inbox: Inbox<LaneProtocol>,
}

impl<I: Debug> Debug for LoadRequest<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadRequest")
            .field("item", &self.item)
            .field("position", &self.position)
            .field("start", &self.start)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaneStart {
    pub speed: SpeedCurve,
    pub keylock: bool,
    pub backend: StretchKind,
}

#[derive(Debug)]
pub struct Loaded<O> {
    pub lane: LaneId,
    pub opened: O,
    pub engine_latency: FrameCount,
}

#[derive(Debug)]
pub enum Dispatched<O> {
    Loaded(Loaded<O>),
    Released,
    Prioritized,
}

/// One source open yielding its receiver and its actual worker-owned lane.
pub trait Open: Debug {
    type Opened: Debug;
    type Lane;

    fn open(
        self,
        position: Duration,
        start: LaneStart,
        inbox: Inbox<LaneProtocol>,
    ) -> impl MaybeSendFuture<Output = Result<(Self::Opened, Self::Lane, FrameCount), LoadRefusal>>;
}

/// Mutable lane state accessed only by the dispatcher that owns the task.
pub trait LaneTask: Task {
    fn set_priority(&mut self, class: ServiceClass);
    fn poll_commands(&mut self, cx: &mut Context<'_>) -> Poll<()>;
}

#[derive(Debug)]
pub struct DispatcherProtocol<I>(PhantomData<fn() -> I>);

impl<I: Open> Protocol for DispatcherProtocol<I> {
    type Applied = Dispatched<I::Opened>;
    type Clock = ();
    type Command = DispatcherCommand<I>;
    type Refusal = LoadRefusal;
    type Target = Infallible;

    fn frames_since((): (), (): ()) -> Option<u64> {
        Some(0)
    }
}

type Opening<O, L> =
    LocalBoxFuture<'static, (Seq, LaneId, Result<(O, L, FrameCount), LoadRefusal>)>;

struct DispatchState<I: Open> {
    inbox: Inbox<DispatcherProtocol<I>>,
    opening: FuturesUnordered<Opening<I::Opened, I::Lane>>,
    lanes: Vec<(LaneId, I::Lane)>,
    capacity: NonZeroUsize,
    outcome: TickResult,
}

impl<I> DispatchState<I>
where
    I: Open + 'static,
    I::Opened: 'static,
    I::Lane: LaneTask,
{
    fn new(inbox: Inbox<DispatcherProtocol<I>>, capacity: NonZeroUsize) -> Self {
        Self {
            inbox,
            opening: FuturesUnordered::new(),
            lanes: Vec::with_capacity(capacity.get()),
            capacity,
            outcome: TickResult::Waiting,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut progress = false;
        while let Poll::Ready(Some((seq, lane, result))) = self.opening.poll_next_unpin(cx) {
            progress = true;
            let Some(due) = self.inbox.resume(seq, (), ()) else {
                continue;
            };
            match result {
                Ok((opened, task, engine_latency)) => {
                    self.lanes.push((lane, task));
                    due.apply(Dispatched::Loaded(Loaded {
                        lane,
                        opened,
                        engine_latency,
                    }));
                }
                Err(refusal) => due.refuse(refusal),
            }
        }
        let _ = self.inbox.poll_drain(cx);
        if self.inbox.is_closed() {
            return Poll::Ready(());
        }
        while let Some(mut due) = self.inbox.next_due((), 1) {
            progress = true;
            if due.commands().len() != 1 {
                continue;
            }
            let Some(command) = due.commands_mut().pop() else {
                continue;
            };
            match command {
                DispatcherCommand::Load(request) => {
                    if self.lanes.len() + self.opening.len() >= self.capacity.get() {
                        due.refuse(LoadRefusal::Capacity {
                            capacity: self.capacity.get(),
                        });
                        continue;
                    }
                    let seq = due.defer();
                    let lane = LaneId(seq.get());
                    self.opening.push(
                        async move {
                            let result = request
                                .item
                                .open(request.position, request.start, request.inbox)
                                .await;
                            (seq, lane, result)
                        }
                        .boxed_local(),
                    );
                }
                DispatcherCommand::Release(lane) => {
                    if let Some(index) = self.lanes.iter().position(|(id, _)| *id == lane) {
                        self.lanes.remove(index);
                        due.apply(Dispatched::Released);
                    } else {
                        due.commands_mut().push(DispatcherCommand::Release(lane));
                    }
                }
                DispatcherCommand::SetPriority(lane, class) => {
                    if let Some((_, task)) = self.lanes.iter_mut().find(|(id, _)| *id == lane) {
                        task.set_priority(class);
                        due.apply(Dispatched::Prioritized);
                    } else {
                        due.commands_mut()
                            .push(DispatcherCommand::SetPriority(lane, class));
                    }
                }
            }
        }
        self.lanes
            .sort_unstable_by(|(left_id, left), (right_id, right)| {
                right
                    .priority()
                    .cmp(&left.priority())
                    .then_with(|| left_id.cmp(right_id))
            });
        let mut waiting = !self.opening.is_empty();
        let mut upstream = false;
        for (_, lane) in &mut self.lanes {
            let _ = lane.poll_commands(cx);
            lane.recycle();
            let result = lane.tick();
            progress |= result == TickResult::Progress;
            waiting |= result == TickResult::Waiting;
            upstream |= result == TickResult::UpstreamPending;
        }
        self.outcome = if progress {
            TickResult::Progress
        } else if waiting {
            TickResult::Waiting
        } else if upstream {
            TickResult::UpstreamPending
        } else {
            TickResult::Backpressured
        };
        if progress {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    fn priority(&self) -> Option<Priority> {
        self.lanes
            .iter()
            .filter_map(|(_, task)| task.priority())
            .max()
    }
}

/// Drive source opens, commands and resident lanes on the current owner thread.
pub async fn dispatch<I>(inbox: Inbox<DispatcherProtocol<I>>)
where
    I: Open + 'static,
    I::Opened: 'static,
    I::Lane: LaneTask,
{
    let mut dispatcher = DispatchState::new(inbox, crate::consts::CAPACITY);
    poll_fn(|cx| dispatcher.poll(cx)).await;
}

pub(crate) struct DispatcherTask<I: Open> {
    dispatcher: DispatchState<I>,
    waker: Waker,
}

impl<I> DispatcherTask<I>
where
    I: Open + 'static,
    I::Opened: 'static,
    I::Lane: LaneTask,
{
    pub(crate) fn new(
        inbox: Inbox<DispatcherProtocol<I>>,
        capacity: NonZeroUsize,
        wake: kithara_worker::Wake,
    ) -> Self {
        Self {
            dispatcher: DispatchState::new(inbox, capacity),
            waker: Waker::from(std::sync::Arc::new(Wake::new(wake))),
        }
    }
}

impl<I> Task for DispatcherTask<I>
where
    I: Open + 'static,
    I::Opened: 'static,
    I::Lane: LaneTask,
{
    fn priority(&self) -> Option<Priority> {
        self.dispatcher.priority()
    }

    fn on_cancel(&mut self) {
        for (_, task) in &mut self.dispatcher.lanes {
            task.on_cancel();
        }
    }

    fn tick(&mut self) -> TickResult {
        let mut context = Context::from_waker(&self.waker);
        if self.dispatcher.poll(&mut context).is_ready() {
            TickResult::Done
        } else {
            self.dispatcher.outcome
        }
    }
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

    struct TestLane;
    impl Task for TestLane {
        fn tick(&mut self) -> TickResult {
            TickResult::Waiting
        }
    }
    impl LaneTask for TestLane {
        fn set_priority(&mut self, _class: ServiceClass) {}
        fn poll_commands(&mut self, _cx: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }

    impl Open for Gate {
        type Opened = u32;
        type Lane = TestLane;

        fn open(
            self,
            _position: Duration,
            _start: LaneStart,
            _inbox: Inbox<LaneProtocol>,
        ) -> impl MaybeSendFuture<Output = Result<(u32, TestLane, FrameCount), LoadRefusal>>
        {
            async move {
                self.0
                    .await
                    .expect("the test answers every open it lets run")
                    .map(|value| (value, TestLane, FrameCount::new(0)))
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

    fn send(sender: &mut Sender<Protocol>, items: Vec<Gate>) -> Seq {
        sender
            .send(
                When::Next,
                Batch {
                    basis: Vec::new(),
                    commands: items
                        .into_iter()
                        .map(|item| {
                            DispatcherCommand::Load(LoadRequest {
                                item,
                                position: Duration::ZERO,
                                start: LaneStart {
                                    speed: SpeedCurve::Constant(1.0),
                                    keylock: false,
                                    backend: StretchKind::default(),
                                },
                                inbox: channel(ChannelConfig::builder().build()).1,
                            })
                        })
                        .collect(),
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
                    && matches!(receipt.outcome(), Outcome::Applied { data: Dispatched::Loaded(Loaded { opened: 2, .. }), .. })
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
