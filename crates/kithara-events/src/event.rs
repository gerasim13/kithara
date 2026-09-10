#![forbid(unsafe_code)]

use kithara_platform::tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::{Envelope, EventBus, EventMeta, TopicReceiver};

/// A value that can travel on its own channel.
///
/// Implemented only through `#[derive(Event)]` outside this crate; the
/// `derivable_event` idiom check denies a hand-written impl anywhere else.
pub trait Event: Clone + core::fmt::Debug + Send + Sync + 'static {}

/// One or more [`Event`] types a consumer wants on a single receiver.
///
/// Every `Event` is a one-member set through the blanket impl below; a
/// multi-member set is a consumer-local enum with `#[derive(EventSet)]`.
pub trait EventSet: Sized + Send + 'static {
    type Receivers: Send;

    fn subscribe(bus: &EventBus) -> Self::Receivers;

    fn recv(
        rx: &mut Self::Receivers,
    ) -> impl Future<Output = Result<Envelope<Self>, RecvError>> + Send;

    /// # Errors
    /// Returns `Empty`, lag information, or `Closed` when all members close.
    fn try_recv(rx: &mut Self::Receivers) -> Result<Envelope<Self>, TryRecvError>;

    fn publish(bus: &EventBus, meta: EventMeta, event: Self);
}

impl<E: Event> EventSet for E {
    type Receivers = TopicReceiver<E>;

    fn subscribe(bus: &EventBus) -> Self::Receivers {
        TopicReceiver::new(bus)
    }

    fn recv(
        rx: &mut Self::Receivers,
    ) -> impl Future<Output = Result<Envelope<Self>, RecvError>> + Send {
        rx.recv()
    }

    fn try_recv(rx: &mut Self::Receivers) -> Result<Envelope<Self>, TryRecvError> {
        rx.try_recv()
    }

    fn publish(bus: &EventBus, meta: EventMeta, event: Self) {
        bus.publish_stamped(meta, event);
    }
}
