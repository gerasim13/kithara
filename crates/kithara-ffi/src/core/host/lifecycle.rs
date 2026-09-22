use kithara::platform::sync::Mutex;

use crate::types::FfiError;

enum State<T> {
    Uninitialized,
    Initializing,
    Ready(T),
}

pub(crate) struct Lifecycle<T> {
    state: Mutex<State<T>>,
}

impl<T> Default for Lifecycle<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::Uninitialized),
        }
    }
}

impl<T> Lifecycle<T> {
    pub(crate) fn initialize(
        &self,
        build: impl FnOnce() -> Result<T, FfiError>,
    ) -> Result<(), FfiError> {
        {
            let mut state = self.state.lock();
            match &*state {
                State::Uninitialized => *state = State::Initializing,
                State::Initializing => return Err(FfiError::InitializationInProgress),
                State::Ready(_) => return Err(FfiError::AlreadyInitialized),
            }
        }

        match build() {
            Ok(value) => {
                *self.state.lock() = State::Ready(value);
                Ok(())
            }
            Err(error) => {
                *self.state.lock() = State::Uninitialized;
                Err(error)
            }
        }
    }

    pub(crate) fn with_ready<R>(&self, f: impl FnOnce(&T) -> R) -> Result<R, FfiError> {
        let state = self.state.lock();
        match &*state {
            State::Ready(value) => Ok(f(value)),
            State::Uninitialized => Err(FfiError::NotInitialized),
            State::Initializing => Err(FfiError::InitializationInProgress),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn with_ready_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, FfiError> {
        let mut state = self.state.lock();
        match &mut *state {
            State::Ready(value) => Ok(f(value)),
            State::Uninitialized => Err(FfiError::NotInitialized),
            State::Initializing => Err(FfiError::InitializationInProgress),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        thread,
    };

    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn failed_initialization_can_retry_but_ready_cannot_reinitialize() {
        let lifecycle = Lifecycle::default();
        assert!(matches!(
            lifecycle.with_ready(|value: &u32| *value),
            Err(FfiError::NotInitialized)
        ));
        assert!(matches!(
            lifecycle.initialize(|| Err(FfiError::InvalidArgument {
                reason: "rejected".to_owned(),
            })),
            Err(FfiError::InvalidArgument { .. })
        ));
        lifecycle.initialize(|| Ok(7)).expect("retry succeeds");
        assert_eq!(lifecycle.with_ready(|value| *value).unwrap(), 7);
        assert!(matches!(
            lifecycle.initialize(|| Ok(9)),
            Err(FfiError::AlreadyInitialized)
        ));
    }

    #[kithara::test]
    fn concurrent_initialization_is_rejected_while_builder_owns_no_lock() {
        let lifecycle = Arc::new(Lifecycle::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_lifecycle = Arc::clone(&lifecycle);
        let worker = thread::spawn(move || {
            worker_lifecycle.initialize(|| {
                entered_tx.send(()).expect("signal builder entry");
                release_rx.recv().expect("release builder");
                Ok(7)
            })
        });
        entered_rx.recv().expect("builder entered");
        assert!(matches!(
            lifecycle.initialize(|| Ok(9)),
            Err(FfiError::InitializationInProgress)
        ));
        release_tx.send(()).expect("release builder");
        worker.join().expect("initializer thread").unwrap();
        assert_eq!(lifecycle.with_ready(|value| *value).unwrap(), 7);
    }
}
