use std::{mem, num::NonZeroUsize};

use kithara_platform::{thread, tokio::sync::oneshot};
use kithara_test_utils::kithara;

use super::{SendError, Sender, channel};
use crate::{
    ChannelConfig, Inbox,
    protocol::{Batch, Clock, Protocol, Seq, Target, When},
    receipt::{Outcome, Rejection},
};

const BLOCK: usize = 64;

type Parts = (Outcome<Test>, Batch<Test>);

#[derive(Debug, PartialEq, Eq)]
enum Test {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot(usize);

impl Target for Slot {
    fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Frame(u64);

impl Clock for Frame {
    fn frames_since(self, start: Self) -> Option<u64> {
        self.0.checked_sub(start.0)
    }
}

impl Protocol for Test {
    type Applied = ();
    type Clock = Frame;
    type Command = u32;
    type Refusal = &'static str;
    type Target = Slot;
}

fn pair(capacity: usize, targets: usize) -> (Sender<Test>, Inbox<Test>) {
    let capacity = NonZeroUsize::new(capacity).expect("a test channel holds a batch");
    channel(
        ChannelConfig::builder()
            .capacity(capacity)
            .targets(targets)
            .build(),
    )
}

fn batch(command: u32, basis: &[(Slot, Option<Seq>)]) -> Batch<Test> {
    Batch {
        basis: basis.to_vec(),
        commands: vec![command],
    }
}

fn send(sender: &mut Sender<Test>, when: When<Frame>, batch: Batch<Test>) -> Seq {
    sender.send(when, batch).expect("the test channel has room")
}

fn run_block(inbox: &mut Inbox<Test>, start: u64, frames: usize) -> Vec<(usize, u32)> {
    inbox.drain();
    let mut applied = Vec::new();
    while let Some(due) = inbox.next_due(Frame(start), frames) {
        let offset = due.offset();
        applied.extend(due.commands().iter().map(|&command| (offset, command)));
        due.apply(());
    }
    applied
}

fn outcomes(sender: &mut Sender<Test>) -> Vec<(Seq, Outcome<Test>)> {
    sender
        .receipts()
        .map(|receipt| {
            let seq = receipt.seq();
            let (outcome, _): Parts = receipt.into();
            (seq, outcome)
        })
        .collect()
}

fn applied(at: u64) -> Outcome<Test> {
    Outcome::Applied {
        at: Frame(at),
        data: (),
    }
}

#[kithara::test]
fn batches_run_in_time_order_then_send_order() {
    let (mut sender, mut inbox) = pair(8, 0);
    send(&mut sender, When::At(Frame(40)), batch(1, &[]));
    send(&mut sender, When::At(Frame(10)), batch(2, &[]));
    send(&mut sender, When::At(Frame(40)), batch(3, &[]));
    send(&mut sender, When::Next, batch(4, &[]));

    assert_eq!(
        run_block(&mut inbox, 0, BLOCK),
        [(0, 4), (10, 2), (40, 1), (40, 3)]
    );
}

#[kithara::test]
fn the_last_frame_is_due_and_the_block_end_waits() {
    let (mut sender, mut inbox) = pair(8, 0);
    send(&mut sender, When::At(Frame(63)), batch(1, &[]));
    send(&mut sender, When::At(Frame(64)), batch(2, &[]));

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(63, 1)]);
    assert_eq!(run_block(&mut inbox, 64, BLOCK), [(0, 2)]);
}

#[kithara::test]
fn a_moment_at_the_end_of_the_clock_waits() {
    let (mut sender, mut inbox) = pair(8, 0);
    send(&mut sender, When::At(Frame(u64::MAX)), batch(1, &[]));

    assert!(run_block(&mut inbox, 0, BLOCK).is_empty());
    assert!(outcomes(&mut sender).is_empty());
}

#[kithara::test]
fn next_applies_at_the_block_start() {
    let (mut sender, mut inbox) = pair(8, 0);
    let seq = send(&mut sender, When::Next, batch(1, &[]));

    assert_eq!(run_block(&mut inbox, 128, BLOCK), [(0, 1)]);
    assert_eq!(outcomes(&mut sender), [(seq, applied(128))]);
}

#[kithara::test]
fn a_moment_before_the_block_is_late_and_returns_whole() {
    let (mut sender, mut inbox) = pair(8, 1);
    let late = send(
        &mut sender,
        When::At(Frame(10)),
        batch(7, &[(Slot(0), None)]),
    );

    assert!(run_block(&mut inbox, 64, BLOCK).is_empty());
    let receipt = sender.receipts().next().expect("the late batch comes back");
    assert_eq!(receipt.seq(), late);
    assert_eq!(receipt.outcome(), &Outcome::Rejected(Rejection::Late));
    let (_, returned): Parts = receipt.into();
    assert_eq!(returned.basis, [(Slot(0), None)]);
    assert_eq!(returned.commands, [7]);

    send(&mut sender, When::Next, batch(8, &[(Slot(0), None)]));
    assert_eq!(run_block(&mut inbox, 128, BLOCK), [(0, 8)]);
}

#[kithara::test]
fn a_skipped_block_returns_its_batches_late_in_time_order() {
    let (mut sender, mut inbox) = pair(8, 0);
    let second = send(&mut sender, When::At(Frame(70)), batch(2, &[]));
    let first = send(&mut sender, When::At(Frame(65)), batch(1, &[]));

    assert!(run_block(&mut inbox, 128, BLOCK).is_empty());
    assert_eq!(
        outcomes(&mut sender),
        [
            (first, Outcome::Rejected(Rejection::Late)),
            (second, Outcome::Rejected(Rejection::Late)),
        ]
    );
}

#[kithara::test]
fn a_full_channel_returns_the_batch_whole() {
    let (mut sender, _inbox) = pair(1, 0);
    send(&mut sender, When::Next, batch(1, &[]));

    let Err(SendError::Full(returned)) = sender.send(When::Next, batch(2, &[])) else {
        panic!("a second batch overflows a channel of one");
    };
    assert_eq!(returned.commands, [2]);
}

#[kithara::test]
fn credits_return_only_with_receipts() {
    let (mut sender, mut inbox) = pair(1, 0);
    send(&mut sender, When::Next, batch(1, &[]));
    inbox.drain();
    assert!(matches!(
        sender.send(When::Next, batch(2, &[])),
        Err(SendError::Full(_))
    ));

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1)]);
    assert!(matches!(
        sender.send(When::Next, batch(3, &[])),
        Err(SendError::Full(_))
    ));

    assert_eq!(outcomes(&mut sender).len(), 1);
    assert_eq!(send(&mut sender, When::Next, batch(4, &[])).get(), 2);
}

#[kithara::test]
fn an_unknown_target_is_refused_before_numbering() {
    let (mut sender, _inbox) = pair(8, 1);

    let Err(SendError::Target(returned)) = sender.send(When::Next, batch(1, &[(Slot(1), None)]))
    else {
        panic!("slot 1 is outside a channel of one target");
    };
    assert_eq!(returned.commands, [1]);
    assert_eq!(
        send(&mut sender, When::Next, batch(2, &[(Slot(0), None)])).get(),
        1
    );
}

#[kithara::test]
fn receipts_follow_execution_order() {
    let (mut sender, mut inbox) = pair(8, 0);
    let a = send(&mut sender, When::At(Frame(30)), batch(1, &[]));
    let b = send(&mut sender, When::At(Frame(10)), batch(2, &[]));
    let c = send(&mut sender, When::At(Frame(20)), batch(3, &[]));

    run_block(&mut inbox, 0, BLOCK);
    let order: Vec<Seq> = outcomes(&mut sender)
        .into_iter()
        .map(|(seq, _)| seq)
        .collect();
    assert_eq!(order, [b, c, a]);
}

#[kithara::test]
fn a_refusal_leaves_the_ledger_untouched() {
    let (mut sender, mut inbox) = pair(8, 1);
    let refused = send(&mut sender, When::Next, batch(1, &[(Slot(0), None)]));
    inbox.drain();
    let due = inbox.next_due(Frame(0), BLOCK).expect("the batch is due");
    due.refuse("busy");

    let retry = send(&mut sender, When::Next, batch(2, &[(Slot(0), None)]));
    assert_eq!(run_block(&mut inbox, 64, BLOCK), [(0, 2)]);
    assert_eq!(
        outcomes(&mut sender),
        [
            (refused, Outcome::Rejected(Rejection::Refused("busy"))),
            (retry, applied(64)),
        ]
    );
}

#[kithara::test]
fn the_executor_returns_resources_inside_the_batch() {
    let (mut sender, mut inbox) = pair(8, 0);
    send(&mut sender, When::Next, batch(7, &[]));
    inbox.drain();

    let mut due = inbox.next_due(Frame(0), BLOCK).expect("the batch is due");
    let taken = mem::replace(&mut due.commands_mut()[0], 99);
    due.apply(());

    assert_eq!(taken, 7);
    let receipt = sender.receipts().next().expect("the receipt arrives");
    let (_, returned): Parts = receipt.into();
    assert_eq!(returned.commands, [99]);
}

#[kithara::test]
fn an_empty_block_applies_nothing() {
    let (mut sender, mut inbox) = pair(8, 0);
    send(&mut sender, When::Next, batch(1, &[]));
    send(&mut sender, When::At(Frame(0)), batch(2, &[]));

    assert!(run_block(&mut inbox, 0, 0).is_empty());
    assert!(outcomes(&mut sender).is_empty());
    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1), (0, 2)]);
}

#[kithara::test(tokio, browser)]
async fn the_halves_cross_threads() {
    let (mut sender, mut inbox) = pair(8, 0);
    let seq = send(&mut sender, When::Next, batch(1, &[]));

    let (done, executed) = oneshot::channel();
    drop(thread::spawn(move || {
        done.send(run_block(&mut inbox, 0, BLOCK))
            .expect("the test awaits the executor");
    }));
    assert_eq!(executed.await.expect("the executor finishes"), [(0, 1)]);
    assert_eq!(outcomes(&mut sender), [(seq, applied(0))]);
}

#[kithara::test]
fn a_current_basis_applies_the_whole_batch_and_chains() {
    let (mut sender, mut inbox) = pair(8, 2);
    let both = Batch {
        basis: vec![(Slot(0), None), (Slot(1), None)],
        commands: vec![1, 2],
    };
    let first = send(&mut sender, When::Next, both);
    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1), (0, 2)]);

    let chained = [(Slot(0), Some(first)), (Slot(1), Some(first))];
    send(&mut sender, When::Next, batch(3, &chained));
    assert_eq!(run_block(&mut inbox, 64, BLOCK), [(0, 3)]);
}

#[kithara::test]
fn a_moved_target_rejects_the_whole_batch() {
    let (mut sender, mut inbox) = pair(8, 2);
    let seek = send(&mut sender, When::Next, batch(1, &[(Slot(0), None)]));
    let both = Batch {
        basis: vec![(Slot(0), None), (Slot(1), None)],
        commands: vec![2, 3],
    };
    let stale = send(&mut sender, When::Next, both);

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1)]);
    assert_eq!(
        outcomes(&mut sender),
        [
            (seek, applied(0)),
            (stale, Outcome::Rejected(Rejection::Stale)),
        ]
    );

    send(&mut sender, When::Next, batch(4, &[(Slot(1), None)]));
    assert_eq!(run_block(&mut inbox, 64, BLOCK), [(0, 4)]);
}

#[kithara::test]
fn a_disjoint_basis_applies_after_another_target_moves() {
    let (mut sender, mut inbox) = pair(8, 2);
    send(&mut sender, When::Next, batch(1, &[(Slot(0), None)]));
    send(&mut sender, When::Next, batch(2, &[(Slot(1), None)]));

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1), (0, 2)]);
}

#[kithara::test]
fn an_empty_basis_applies_and_records_nothing() {
    let (mut sender, mut inbox) = pair(8, 1);
    let seek = send(&mut sender, When::Next, batch(1, &[(Slot(0), None)]));
    send(&mut sender, When::Next, batch(2, &[]));
    send(&mut sender, When::Next, batch(3, &[(Slot(0), Some(seek))]));

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1), (0, 2), (0, 3)]);
}

#[kithara::test]
fn a_held_batch_is_judged_when_it_fires() {
    let (mut sender, mut inbox) = pair(8, 1);
    let start = send(&mut sender, When::Next, batch(1, &[(Slot(0), None)]));
    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1)]);

    let crossfade = send(
        &mut sender,
        When::At(Frame(200)),
        batch(2, &[(Slot(0), Some(start))]),
    );
    assert!(run_block(&mut inbox, 64, BLOCK).is_empty());
    let seek = send(&mut sender, When::Next, batch(3, &[(Slot(0), Some(start))]));
    assert_eq!(run_block(&mut inbox, 128, BLOCK), [(0, 3)]);
    assert!(run_block(&mut inbox, 192, BLOCK).is_empty());

    assert_eq!(
        outcomes(&mut sender),
        [
            (start, applied(0)),
            (seek, applied(128)),
            (crossfade, Outcome::Rejected(Rejection::Stale)),
        ]
    );
}

#[kithara::test]
fn a_repeated_target_applies_only_when_its_entries_agree() {
    let (mut sender, mut inbox) = pair(8, 1);
    let first = send(
        &mut sender,
        When::Next,
        batch(1, &[(Slot(0), None), (Slot(0), None)]),
    );
    send(
        &mut sender,
        When::Next,
        batch(2, &[(Slot(0), Some(first)), (Slot(0), None)]),
    );
    send(
        &mut sender,
        When::Next,
        batch(3, &[(Slot(0), Some(first)), (Slot(0), Some(first))]),
    );

    assert_eq!(run_block(&mut inbox, 0, BLOCK), [(0, 1), (0, 3)]);
}

#[kithara::test]
fn a_dropped_due_batch_comes_back_unanswered() {
    let (mut sender, mut inbox) = pair(1, 0);
    let seq = send(&mut sender, When::Next, batch(1, &[]));
    inbox.drain();
    drop(inbox.next_due(Frame(0), BLOCK));
    let receipt = sender
        .receipts()
        .next()
        .expect("the dropped batch is answered");
    assert_eq!(receipt.seq(), seq);
    let (outcome, returned): Parts = receipt.into();
    assert_eq!(outcome, Outcome::Rejected(Rejection::Unanswered));
    assert_eq!(returned.commands, [1]);
    assert!(sender.send(When::Next, batch(2, &[])).is_ok());
}
