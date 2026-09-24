use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::node::{Node, Slot};
use crate::sync::Arc;

/// Future returned by [`CancelToken::cancelled`](super::CancelToken::cancelled).
/// Resolves once the token's subtree is cancelled. `Unpin`; cancel-safe (drop
/// unregisters its slot).
#[derive(derive_more::Debug)]
pub struct Cancelled<'a> {
    #[debug(skip)]
    node: &'a Arc<Node>,
    #[debug(skip)]
    slot: Option<u64>,
    #[debug(skip)]
    done: bool,
}

impl<'a> Cancelled<'a> {
    pub(super) const fn new(node: &'a Arc<Node>) -> Self {
        Self {
            node,
            slot: None,
            done: false,
        }
    }
}

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let me = self.get_mut();
        poll_cancelled(
            &mut Fields {
                node: me.node,
                slot: &mut me.slot,
                done: &mut me.done,
            },
            cx,
        )
    }
}

impl Drop for Cancelled<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.slot.take() {
            self.node.unregister(id);
        }
    }
}

struct Fields<'a> {
    node: &'a Arc<Node>,
    slot: &'a mut Option<u64>,
    done: &'a mut bool,
}

/// Registers first, since cancel may have fired between the earlier `is_cancelled` check and
/// registration; `register` returns `None` when already fired, which this resolves immediately
/// rather than leaving parked.
fn poll_cancelled(f: &mut Fields<'_>, cx: &mut Context<'_>) -> Poll<()> {
    if *f.done {
        return Poll::Ready(());
    }
    if f.node.is_cancelled() {
        if let Some(id) = f.slot.take() {
            f.node.unregister(id);
        }
        *f.done = true;
        return Poll::Ready(());
    }
    if let Some(id) = *f.slot {
        f.node.refresh_task(id, cx.waker());
        return Poll::Pending;
    }
    if let Some(id) = f.node.register(Slot::Task(cx.waker().clone())) {
        *f.slot = Some(id);
        Poll::Pending
    } else {
        *f.done = true;
        Poll::Ready(())
    }
}
