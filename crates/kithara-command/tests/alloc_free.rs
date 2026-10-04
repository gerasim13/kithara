#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroUsize;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kithara_command::{
    Batch, ChannelConfig, Outcome, Protocol, Rejection, Seq, Target, When, channel,
};
use kithara_test_utils::kithara;

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(15);
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

impl Protocol for Test {
    type Applied = ();
    type Clock = Frame;
    type Command = u32;
    type Refusal = &'static str;
    type Target = Slot;

    fn frames_since(at: Frame, start: Frame) -> Option<u64> {
        at.0.checked_sub(start.0)
    }
}

fn batch(command: u32, basis: &[(Slot, Option<Seq>)]) -> Batch<Test> {
    Batch {
        basis: basis.to_vec(),
        commands: vec![command],
    }
}

#[kithara::test(native)]
fn draining_judging_and_answering_never_allocate() {
    const START: Frame = Frame(64);
    const REFUSED: u32 = 4;
    const UNANSWERED: u32 = 6;

    let config = ChannelConfig::builder()
        .capacity(CAPACITY)
        .targets(2)
        .build();
    let (mut sender, mut inbox) = channel::<Test>(config);
    let marked = [
        (When::Next, batch(1, &[(Slot(0), None)])),
        (When::At(Frame(0)), batch(2, &[])),
        (When::At(Frame(80)), batch(3, &[(Slot(0), None)])),
        (When::At(Frame(90)), batch(REFUSED, &[(Slot(1), None)])),
        (When::At(Frame(1000)), batch(5, &[])),
        (When::At(Frame(100)), batch(UNANSWERED, &[])),
    ];
    let fillers = (10..).map(|command| (When::Next, batch(command, &[])));
    for (when, batch) in marked.into_iter().chain(fillers).take(CAPACITY.get()) {
        sender.send(when, batch).expect("the channel has room");
    }

    assert_no_alloc(|| {
        inbox.drain();
        while let Some(due) = inbox.next_due(START, BLOCK) {
            if due.commands().contains(&REFUSED) {
                due.refuse("busy");
            } else if due.commands().contains(&UNANSWERED) {
                drop(due);
            } else {
                due.apply(());
            }
        }
    });

    let outcomes: Vec<Outcome<Test>> = sender
        .receipts()
        .map(|receipt| Parts::from(receipt).0)
        .collect();
    let applied = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Applied { .. }))
        .count();
    assert_eq!(outcomes.len(), CAPACITY.get() - 1);
    assert_eq!(applied, CAPACITY.get() - 5);
    assert!(outcomes.contains(&Outcome::Rejected(Rejection::Late)));
    assert!(outcomes.contains(&Outcome::Rejected(Rejection::Stale)));
    assert!(outcomes.contains(&Outcome::Rejected(Rejection::Refused("busy"))));
    assert!(outcomes.contains(&Outcome::Rejected(Rejection::Unanswered)));
}
