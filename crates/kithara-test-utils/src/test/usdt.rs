use std::sync::{Mutex, MutexGuard, PoisonError};

use kithara_platform::sync::{Arc, Notify};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, Layer};

#[derive(Clone, Debug, Default)]
pub struct ProbeEvent {
    pub target: String,
    pub probe: String,
    pub fields: Vec<(String, u64)>,
}

impl ProbeEvent {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<u64> {
        self.fields
            .iter()
            .find_map(|(key, value)| (key == name).then_some(*value))
    }
}

#[derive(Default)]
struct State {
    events: Vec<ProbeEvent>,
}

static EVENTS: Mutex<State> = Mutex::new(State { events: Vec::new() });
static SCOPE: Mutex<()> = Mutex::new(());
static RECORDED: Mutex<Option<Arc<Notify>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Scope {
    recorded: Arc<Notify>,
    _serial: MutexGuard<'static, ()>,
}

#[must_use]
pub fn scope() -> Scope {
    let serial = lock(&SCOPE);
    lock(&EVENTS).events.clear();
    let recorded = Arc::new(Notify::default());
    *lock(&RECORDED) = Some(Arc::clone(&recorded));
    Scope {
        recorded,
        _serial: serial,
    }
}

impl Scope {
    #[must_use]
    pub fn events(&self) -> Vec<ProbeEvent> {
        lock(&EVENTS).events.clone()
    }

    /// Resolves once the probes recorded so far satisfy `holds`, re-checking
    /// after every newly recorded probe.
    pub async fn wait_for<F>(&self, mut holds: F)
    where
        F: FnMut(&[ProbeEvent]) -> bool,
    {
        loop {
            let recorded = self.recorded.notified();
            if holds(&lock(&EVENTS).events) {
                return;
            }
            recorded.await;
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        lock(&RECORDED).take();
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
        let mut visitor = ProbeVisitor::default();
        event.record(&mut visitor);
        let Some(probe) = visitor.probe else {
            return;
        };
        lock(&EVENTS).events.push(ProbeEvent {
            target: event.metadata().target().to_owned(),
            probe,
            fields: visitor.fields,
        });
        if let Some(recorded) = lock(&RECORDED).as_ref() {
            recorded.notify_one();
        }
    }
}

#[derive(Default)]
struct ProbeVisitor {
    probe: Option<String>,
    fields: Vec<(String, u64)>,
}

impl Visit for ProbeVisitor {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "probe" {
            self.probe = Some(value.to_owned());
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.push((field.name().to_owned(), value));
    }
}
