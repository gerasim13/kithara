#![cfg(not(target_arch = "wasm32"))]

use kithara::{events::EventBus, hls::HlsEvent};
use kithara_integration_tests::event::TestEvent;

#[kithara::test]
fn test_event_bus_publish_subscribe() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    bus.publish(HlsEvent::EndOfStream);

    let event = rx.try_recv().map(|env| env.event).ok();
    assert!(matches!(event, Some(TestEvent::Hls(HlsEvent::EndOfStream))));
}
