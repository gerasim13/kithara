use std::{
    collections::VecDeque,
    fmt::{self, Write as _},
};

use kithara_platform::sync::{Mutex, OnceLock};
use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    subscriber::Interest,
};
use tracing_subscriber::layer::{Context, Layer};

/// In-memory flight recorder: the last kithara events, whatever the fmt
/// layer's filter admits to stdout. A red test's dump (panic or hang)
/// carries this tail, so the evidence does not depend on guessing the right
/// per-test tracing filter in advance.
///
/// Two lanes so volume cannot evict signal: `#[kithara::probe]` sites fire
/// on every call and would flush rare FSM-transition DEBUG lines out of a
/// shared ring within milliseconds.
const MAX_EVENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 512;

static EVENTS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static PROBES: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

/// The platform mutex has no `const` constructor (the loom backend cannot
/// model one), so the rings initialize lazily.
fn ring(cell: &'static OnceLock<Mutex<VecDeque<String>>>) -> &'static Mutex<VecDeque<String>> {
    cell.get_or_init(|| Mutex::new(VecDeque::new()))
}

#[must_use]
pub fn layer() -> RingLayer {
    RingLayer
}

/// Snapshot of the recorded DEBUG-event tail, oldest first.
#[must_use]
pub fn tail() -> Vec<String> {
    snapshot(ring(&EVENTS))
}

/// Snapshot of the recorded `#[kithara::probe]` tail, oldest first.
#[must_use]
pub fn probes_tail() -> Vec<String> {
    snapshot(ring(&PROBES))
}

fn snapshot(ring: &Mutex<VecDeque<String>>) -> Vec<String> {
    ring.lock().iter().cloned().collect()
}

pub struct RingLayer;

enum Lane {
    Event,
    Probe,
}

/// Only kithara traffic: probe events (TRACE, `<crate>_probe` targets, one
/// per call of a probed production fn) and DEBUG-and-coarser events. The
/// recorder must keep callsites alive that the fmt filter disabled, without
/// pulling in dependency chatter or non-probe TRACE firehoses.
fn classify(meta: &Metadata<'_>) -> Option<Lane> {
    if !meta.is_event() || !meta.target().starts_with("kithara") {
        return None;
    }
    if meta.target().ends_with("_probe") {
        return Some(Lane::Probe);
    }
    (*meta.level() <= Level::DEBUG).then_some(Lane::Event)
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if classify(meta).is_some() {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        classify(meta).is_some()
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Sibling layers may enable events this one does not want; `on_event`
        // is delivered for every globally enabled event.
        let Some(lane) = classify(meta) else {
            return;
        };
        // No timestamp: the ring is chronological by construction, and a wall
        // stamp would mix real and flash-virtual clocks depending on where the
        // event fired.
        let mut line = format!(
            "{level} {target}:",
            level = meta.level(),
            target = meta.target(),
        );
        event.record(&mut LineVisitor { line: &mut line });
        let cell = match lane {
            Lane::Event => &EVENTS,
            Lane::Probe => &PROBES,
        };
        record(ring(cell), line);
    }
}

fn record(ring: &Mutex<VecDeque<String>>, mut line: String) {
    if line.len() > MAX_EVENT_BYTES {
        let mut end = MAX_EVENT_BYTES;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    let mut ring = ring.lock();
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
    use kithara_platform::sync::{Mutex, MutexGuard, OnceLock};
    use tracing::debug;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{MAX_EVENT_BYTES, MAX_EVENTS, layer, probes_tail, tail};
    use crate::kithara;

    /// The rings are process-global and the capacity tests flood them, so a
    /// concurrent marker lookup would find its entry evicted. Each test holds
    /// this lock from its emissions through its assertions.
    static FLIGHT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn with_recorder(body: impl FnOnce()) -> MutexGuard<'static, ()> {
        let guard = FLIGHT_TEST_LOCK.get_or_init(|| Mutex::new(())).lock();
        let subscriber = tracing_subscriber::registry().with(layer());
        tracing::subscriber::with_default(subscriber, body);
        guard
    }

    #[kithara::test]
    fn debug_event_lands_in_the_tail_with_fields() {
        let _guard = with_recorder(|| {
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
        let _guard = with_recorder(|| {
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
            "non-probe TRACE must not be recorded"
        );
    }

    #[kithara::test]
    fn probe_trace_events_land_in_their_own_lane() {
        let _guard = with_recorder(|| {
            tracing::trace!(
                target: "kithara_flight_test_probe",
                probe = "flight_probe_marker",
                seq = 7u64,
                "probe firing"
            );
        });

        let line = probes_tail()
            .into_iter()
            .rev()
            .find(|line| line.contains("flight_probe_marker"))
            .expect("probe firing must be in the probe tail");
        assert!(line.contains("seq=7"), "{line}");
        assert!(
            !tail()
                .iter()
                .any(|line| line.contains("flight_probe_marker")),
            "probe volume must not occupy the event lane"
        );
    }

    #[kithara::test]
    fn probe_volume_does_not_evict_events() {
        let _guard = with_recorder(|| {
            debug!(target: "kithara_flight_test", "rare transition marker");
            for seq in 0..=MAX_EVENTS {
                tracing::trace!(
                    target: "kithara_flight_test_probe",
                    probe = "flood",
                    seq = seq as u64,
                    "probe flood"
                );
            }
        });

        assert!(
            tail()
                .iter()
                .any(|line| line.contains("rare transition marker")),
            "a probe flood must not evict the rare DEBUG event"
        );
    }

    #[kithara::test]
    fn ring_keeps_only_the_newest_events() {
        let _guard = with_recorder(|| {
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
        let _guard = with_recorder(|| {
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
