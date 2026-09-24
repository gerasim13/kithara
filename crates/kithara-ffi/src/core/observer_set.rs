use kithara::platform::sync::{Arc, Mutex};

use crate::{observer::ItemObserver, types::FfiItemEvent};

#[derive(Default)]
struct Registrations {
    entries: Vec<(u64, Arc<dyn ItemObserver>)>,
    next_id: u64,
}

#[derive(Default)]
pub(crate) struct ObserverSet {
    registrations: Mutex<Registrations>,
}

impl ObserverSet {
    pub(crate) fn add(&self, observer: Arc<dyn ItemObserver>) -> u64 {
        let mut registrations = self.registrations.lock();
        let id = registrations.next_id;
        registrations.next_id += 1;
        registrations.entries.push((id, observer));
        id
    }

    pub(crate) fn remove(&self, id: u64) {
        self.registrations
            .lock()
            .entries
            .retain(|(known, _)| *known != id);
    }

    /// Observers are called outside the lock: one may unsubscribe from
    /// inside its callback.
    fn snapshot(&self) -> Vec<Arc<dyn ItemObserver>> {
        self.registrations
            .lock()
            .entries
            .iter()
            .map(|(_, observer)| Arc::clone(observer))
            .collect()
    }
}

impl ItemObserver for ObserverSet {
    fn on_event(&self, event: FfiItemEvent) {
        let observers = self.snapshot();
        let Some((last, rest)) = observers.split_last() else {
            return;
        };
        for observer in rest {
            observer.on_event(event.clone());
        }
        last.on_event(event);
    }
}
