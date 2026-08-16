use std::{
    collections::VecDeque,
    fmt::{self, Write as _},
    sync::{Mutex, PoisonError},
};

use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    subscriber::Interest,
};
use tracing_subscriber::layer::{Context, Layer};

/// In-memory flight recorder: the last kithara DEBUG-and-coarser events,
/// whatever the fmt layer's filter admits to stdout. A red test's dump
/// (panic or hang) carries this tail, so the evidence does not depend on
/// guessing the right per-test tracing filter in advance.
const MAX_EVENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 512;

static RING: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

#[must_use]
pub fn layer() -> RingLayer {
    RingLayer
}

/// Snapshot of the recorded tail, oldest first.
#[must_use]
pub fn tail() -> Vec<String> {
    let ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
    ring.iter().cloned().collect()
}

pub struct RingLayer;

/// Only kithara events at DEBUG or coarser: the recorder must keep callsites
/// alive that the fmt filter disabled, without pulling in dependency chatter
/// or TRACE firehoses.
fn wanted(meta: &Metadata<'_>) -> bool {
    meta.is_event() && *meta.level() <= Level::DEBUG && meta.target().starts_with("kithara")
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if wanted(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        wanted(meta)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Sibling layers may enable events this one does not want; `on_event`
        // is delivered for every globally enabled event.
        if !wanted(meta) {
            return;
        }
        // No timestamp: the ring is chronological by construction, and a wall
        // stamp would mix real and flash-virtual clocks depending on where the
        // event fired.
        let mut line = format!(
            "{level} {target}:",
            level = meta.level(),
            target = meta.target(),
        );
        event.record(&mut LineVisitor { line: &mut line });
        record(line);
    }
}

fn record(mut line: String) {
    if line.len() > MAX_EVENT_BYTES {
        let mut end = MAX_EVENT_BYTES;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    let mut ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
    if ring.len() == MAX_EVENTS {
        ring.pop_front();
    }
    ring.push_back(line);
}

struct LineVisitor<'a> {
    line: &'a mut String,
}

impl Visit for LineVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.line, " {value:?}");
        } else {
            let _ = write!(self.line, " {name}={value:?}", name = field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing::debug;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{MAX_EVENT_BYTES, MAX_EVENTS, layer, tail};
    use crate::kithara;

    fn with_recorder(body: impl FnOnce()) {
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, body);
    }

    #[kithara::test]
    fn debug_event_lands_in_the_tail_with_fields() {
        with_recorder(|| {
            debug!(target: "kithara_flight_test", marker = 41, "unique recorder probe");
        });

        let line = tail()
            .into_iter()
            .rev()
            .find(|line| line.contains("unique recorder probe"))
            .expect("recorded event must be in the tail");
        assert!(line.contains("kithara_flight_test"), "{line}");
        assert!(line.contains("marker=41"), "{line}");
    }

    #[kithara::test]
    fn non_kithara_and_trace_events_are_not_recorded() {
        with_recorder(|| {
            debug!(target: "hyper_util::client", "foreign dependency chatter");
            tracing::trace!(target: "kithara_flight_test", "too fine for the ring");
        });

        let recorded = tail();
        assert!(
            !recorded
                .iter()
                .any(|line| line.contains("foreign dependency chatter")),
            "foreign target must not be recorded"
        );
        assert!(
            !recorded
                .iter()
                .any(|line| line.contains("too fine for the ring")),
            "TRACE must not be recorded"
        );
    }

    #[kithara::test]
    fn ring_keeps_only_the_newest_events() {
        with_recorder(|| {
            for index in 0..=MAX_EVENTS {
                debug!(target: "kithara_flight_test", index, "capacity probe");
            }
        });

        let recorded = tail();
        assert!(recorded.len() <= MAX_EVENTS);
        assert!(
            recorded.iter().any(|line| {
                line.contains("capacity probe") && line.contains(&format!("index={MAX_EVENTS}"))
            }),
            "newest event must survive"
        );
    }

    #[kithara::test]
    fn oversized_event_is_truncated_at_a_char_boundary() {
        let payload = "п".repeat(MAX_EVENT_BYTES);
        with_recorder(|| {
            debug!(target: "kithara_flight_test", oversized = payload.as_str(), "boundary probe");
        });

        let line = tail()
            .into_iter()
            .rev()
            .find(|line| line.contains("boundary probe"))
            .expect("truncated event must still be recorded");
        assert!(line.len() <= MAX_EVENT_BYTES + '…'.len_utf8());
        assert!(line.ends_with('…'), "{line}");
    }
}
