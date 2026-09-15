use std::{
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, ThreadId},
};

use kithara_platform::sync::{Arc, Notify};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, Layer};

/// Payload fields one probe firing can carry: the USDT provider's six `u64`
/// arguments minus the operation id.
const MAX_FIELDS: usize = 5;

/// Firings a scope's history holds. Past it the history stops growing and
/// every reader of it fails, so a probe-heavy run can neither swell the
/// process nor hand a census a silently truncated history. The latest firing
/// of each probe stays readable either way.
pub const MAX_EVENTS: usize = 1 << 19;

/// One recorded firing. Every string is `'static`, so recording a firing
/// allocates nothing beyond the history itself.
#[derive(Clone, Copy, Debug)]
pub struct ProbeEvent {
    pub target: &'static str,
    pub probe: &'static str,
    /// Source file of the product function the probe is attached to.
    pub file: Option<&'static str>,
    /// Source line of that function.
    pub line: Option<u32>,
    /// Thread the probe fired on.
    pub thread: ThreadId,
    fields: [(&'static str, u64); MAX_FIELDS],
    len: usize,
}

impl ProbeEvent {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<u64> {
        self.fields[..self.len]
            .iter()
            .find_map(|(key, value)| (*key == name).then_some(*value))
    }
}

struct State {
    /// Probe names seen by this process. Bounded by the probe call sites, and
    /// kept across scopes so each name is interned once.
    names: Vec<&'static str>,
    events: Vec<ProbeEvent>,
    latest: Vec<ProbeEvent>,
    overflowed: bool,
    recorded: Option<Arc<Notify>>,
}

impl State {
    fn intern(&mut self, probe: &str) -> &'static str {
        if let Some(name) = self.names.iter().find(|name| **name == probe) {
            return name;
        }
        let name: &'static str = Box::leak(probe.into());
        self.names.push(name);
        name
    }

    fn history(&self) -> &[ProbeEvent] {
        assert!(
            !self.overflowed,
            "usdt scope history overflowed {MAX_EVENTS} firings; \
             scope the observation to the operation under test"
        );
        &self.events
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    names: Vec::new(),
    events: Vec::new(),
    latest: Vec::new(),
    overflowed: false,
    recorded: None,
});
static SCOPE: Mutex<()> = Mutex::new(());
/// Lock-free gate for the probe hot path: set only while a [`Scope`] lives.
static ARMED: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Scope {
    recorded: Arc<Notify>,
    _serial: MutexGuard<'static, ()>,
}

/// Records every probe firing until the scope drops.
#[must_use]
pub fn scope() -> Scope {
    let serial = lock(&SCOPE);
    let recorded = Arc::new(Notify::default());
    {
        let mut state = lock(&STATE);
        state.events = Vec::new();
        state.latest = Vec::new();
        state.overflowed = false;
        state.recorded = Some(Arc::clone(&recorded));
    }
    ARMED.store(true, Ordering::Release);
    Scope {
        recorded,
        _serial: serial,
    }
}

/// Every firing recorded by the live [`Scope`], in order.
#[must_use]
pub fn events() -> Vec<ProbeEvent> {
    lock(&STATE).history().to_vec()
}

/// The latest firing of `probe` recorded by the live [`Scope`].
#[must_use]
pub fn last(probe: &str) -> Option<ProbeEvent> {
    lock(&STATE)
        .latest
        .iter()
        .find(|event| event.probe == probe)
        .copied()
}

impl Scope {
    #[must_use]
    pub fn events(&self) -> Vec<ProbeEvent> {
        events()
    }

    #[must_use]
    pub fn last(&self, probe: &str) -> Option<ProbeEvent> {
        last(probe)
    }

    /// Resolves once the firings recorded so far satisfy `holds`,
    /// re-checking after every newly recorded firing.
    pub async fn wait_for<F>(&self, mut holds: F)
    where
        F: FnMut(&[ProbeEvent]) -> bool,
    {
        loop {
            let recorded = self.recorded.notified();
            if holds(lock(&STATE).history()) {
                return;
            }
            recorded.await;
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        ARMED.store(false, Ordering::Release);
        let mut state = lock(&STATE);
        state.recorded = None;
        state.events = Vec::new();
        state.latest = Vec::new();
    }
}

#[must_use]
pub fn layer() -> UsdtLayer {
    UsdtLayer
}

pub struct UsdtLayer;

impl<S: Subscriber> Layer<S> for UsdtLayer {
    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        meta.is_event() && meta.target().ends_with("_probe")
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !ARMED.load(Ordering::Acquire) {
            return;
        }
        let mut visitor = ProbeVisitor {
            probe: None,
            fields: [("", 0); MAX_FIELDS],
            len: 0,
        };
        let mut state = lock(&STATE);
        let Some(recorded) = state.recorded.clone() else {
            return;
        };
        event.record(&mut FieldVisitor {
            state: &mut state,
            visitor: &mut visitor,
        });
        let Some(probe) = visitor.probe else {
            return;
        };
        let metadata = event.metadata();
        let recorded_event = ProbeEvent {
            target: metadata.target(),
            probe,
            file: metadata.file(),
            line: metadata.line(),
            thread: thread::current().id(),
            fields: visitor.fields,
            len: visitor.len,
        };
        if state.events.len() == MAX_EVENTS {
            state.overflowed = true;
        } else {
            state.events.push(recorded_event);
        }
        match state.latest.iter_mut().find(|event| event.probe == probe) {
            Some(slot) => *slot = recorded_event,
            None => state.latest.push(recorded_event),
        }
        drop(state);
        recorded.notify_one();
    }
}

struct ProbeVisitor {
    probe: Option<&'static str>,
    fields: [(&'static str, u64); MAX_FIELDS],
    len: usize,
}

struct FieldVisitor<'a> {
    state: &'a mut State,
    visitor: &'a mut ProbeVisitor,
}

impl FieldVisitor<'_> {
    fn push(&mut self, field: &Field, value: u64) {
        let visitor = &mut *self.visitor;
        if let Some(slot) = visitor.fields.get_mut(visitor.len) {
            *slot = (field.name(), value);
            visitor.len += 1;
        }
    }
}

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "probe" {
            self.visitor.probe = Some(self.state.intern(value));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, value);
    }

    /// Session-axis frames arrive as `i64`; a negative one has no `u64`
    /// reading, so it stays absent and the reader asking for it fails loudly.
    fn record_i64(&mut self, field: &Field, value: i64) {
        if let Ok(value) = u64::try_from(value) {
            self.push(field, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_EVENTS, scope};

    fn fire(probe: &'static str, value: u64) {
        tracing::event!(target: "kithara_test_probe", tracing::Level::TRACE, probe = probe, value = value);
    }

    #[test]
    fn a_scope_records_every_probe_in_order() {
        crate::test::setup_tracing();
        let trace = scope();
        fire("first", 1);
        fire("second", 2);

        let events = trace.events();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].probe, "first");
        assert_eq!(events[1].field("value"), Some(2));
    }

    #[test]
    fn an_overflowing_history_keeps_the_latest_firing() {
        crate::test::setup_tracing();
        let trace = scope();
        for value in 0..=MAX_EVENTS as u64 {
            fire("tick", value);
        }

        assert_eq!(
            trace.last("tick").and_then(|event| event.field("value")),
            Some(MAX_EVENTS as u64)
        );
    }

    #[test]
    #[should_panic(expected = "overflowed")]
    fn an_overflowing_history_fails_its_reader() {
        crate::test::setup_tracing();
        let trace = scope();
        for value in 0..=MAX_EVENTS as u64 {
            fire("tick", value);
        }

        let _ = trace.events();
    }
}
