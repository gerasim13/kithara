use kithara::{
    abr::AbrMode,
    events::{AbrEvent, AudioEvent, Envelope, Event},
    platform::{
        CancelToken,
        sync::{Arc, Mutex},
        tokio,
        tokio::sync::broadcast,
    },
};

use crate::{
    item::ItemView,
    observer::ItemObserver,
    types::{FfiError, FfiItemEvent, FfiItemStatus},
};

pub(crate) struct ItemEventBridge {
    cancel: CancelToken,
}

impl ItemEventBridge {
    /// Milliseconds per second.
    const MS_PER_SECOND: f64 = 1000.0;

    /// 2^32 for splitting u64 into two u32 halves for lossless f64 conversion.
    const U32_MAX_PLUS_ONE: f64 = 4_294_967_296.0;

    /// Bit shift width for extracting the high 32 bits of a u64.
    const U64_HIGH_SHIFT: u32 = 32;

    /// Threshold for suppressing redundant duration/buffered updates (seconds).
    const UPDATE_THRESHOLD: f64 = 0.01;

    fn dispatch(
        observer: &Arc<dyn ItemObserver>,
        event: &Event,
        duration_seconds: &mut Option<f64>,
        variants: &mut Vec<crate::types::FfiVariant>,
        state: &Arc<Mutex<ItemView>>,
    ) {
        if let Some(duration) = Self::duration_from_event(event)
            && duration_seconds
                .is_none_or(|current| (current - duration).abs() > Self::UPDATE_THRESHOLD)
        {
            *duration_seconds = Some(duration);
            state.lock().resolve_duration(duration);
            observer.on_event(FfiItemEvent::DurationChanged { seconds: duration });
        }

        Self::dispatch_variant_events(observer, event, variants);

        if let Ok(event) = FfiItemEvent::try_from(event) {
            observer.on_event(event);
        }

        if let Ok(error) = FfiError::try_from(event)
            && state.lock().mark_failed()
        {
            observer.on_event(FfiItemEvent::StatusChanged {
                status: FfiItemStatus::Failed,
            });
            observer.on_event(FfiItemEvent::Error {
                error: error.to_string(),
            });
        }
    }

    fn dispatch_variant_events(
        observer: &Arc<dyn ItemObserver>,
        event: &Event,
        variants: &mut Vec<crate::types::FfiVariant>,
    ) {
        match event {
            Event::Abr(AbrEvent::VariantsRegistered {
                variants: v,
                initial,
            }) => {
                let ffi_variants: Vec<crate::types::FfiVariant> = v
                    .iter()
                    .filter_map(|vi| {
                        let Ok(index) = u32::try_from(vi.variant_index.get()) else {
                            tracing::error!(
                                idx = vi.variant_index.get(),
                                "BUG: HLS variant index exceeds u32::MAX, dropped from FFI list"
                            );
                            return None;
                        };
                        Some(crate::types::FfiVariant {
                            index,
                            bandwidth_bps: vi.bandwidth_bps.unwrap_or(0),
                            name: vi.name.clone(),
                        })
                    })
                    .collect();
                variants.clone_from(&ffi_variants);
                observer.on_event(FfiItemEvent::VariantsDiscovered {
                    variants: ffi_variants,
                });
                let Ok(initial_u32) = u32::try_from(initial.get()) else {
                    tracing::error!(
                        idx = initial.get(),
                        "BUG: initial HLS variant index exceeds u32::MAX, skipping initial VariantApplied"
                    );
                    return;
                };
                if let Some(initial) = variants.iter().find(|v| v.index == initial_u32) {
                    observer.on_event(FfiItemEvent::VariantApplied {
                        variant: initial.clone(),
                    });
                }
            }
            Event::Abr(AbrEvent::ModeChanged {
                mode: AbrMode::Manual(idx),
            }) => {
                let Ok(idx_u32) = u32::try_from(idx.get()) else {
                    tracing::error!(
                        idx = idx.get(),
                        "BUG: manual variant index exceeds u32::MAX, skipping VariantSelected"
                    );
                    return;
                };
                let variant = variants
                    .iter()
                    .find(|v| v.index == idx_u32)
                    .cloned()
                    .unwrap_or(crate::types::FfiVariant {
                        index: idx_u32,
                        bandwidth_bps: 0,
                        name: None,
                    });
                observer.on_event(FfiItemEvent::VariantSelected { variant });
            }
            Event::Abr(AbrEvent::VariantApplied { to, .. }) => {
                let Ok(idx_u32) = u32::try_from(to.get()) else {
                    tracing::error!(
                        idx = to.get(),
                        "BUG: applied variant index exceeds u32::MAX, skipping VariantApplied"
                    );
                    return;
                };
                let variant = variants
                    .iter()
                    .find(|v| v.index == idx_u32)
                    .cloned()
                    .unwrap_or(crate::types::FfiVariant {
                        index: idx_u32,
                        bandwidth_bps: 0,
                        name: None,
                    });
                observer.on_event(FfiItemEvent::VariantApplied { variant });
            }
            _ => {}
        }
    }

    fn duration_from_event(event: &Event) -> Option<f64> {
        match event {
            Event::Audio(AudioEvent::PlaybackProgress {
                total_ms: Some(total_ms),
                ..
            }) => Some(Self::u64_to_f64(*total_ms)? / Self::MS_PER_SECOND),
            _ => None,
        }
    }

    /// Spawn a task that translates resource events into item callbacks
    /// and refreshes the shared [`ItemView`] cache backing the item's
    /// synchronous getters (`duration_sec`, `is_live_stream`, …).
    pub(crate) fn spawn(
        rx: kithara::events::EventReceiver,
        observer: Arc<dyn ItemObserver>,
        duration_seconds: Option<f64>,
        state: Arc<Mutex<ItemView>>,
        cancel: CancelToken,
    ) -> Self {
        if let Some(duration) = duration_seconds {
            state.lock().resolve_duration(duration);
            observer.on_event(FfiItemEvent::DurationChanged { seconds: duration });
        }
        Self::spawn_event_task(rx, observer, duration_seconds, state, cancel.clone());
        Self { cancel }
    }

    fn spawn_event_task(
        mut rx: kithara::events::EventReceiver,
        observer: Arc<dyn ItemObserver>,
        mut duration_seconds: Option<f64>,
        state: Arc<Mutex<ItemView>>,
        cancel: CancelToken,
    ) {
        crate::FFI_RUNTIME.spawn(async move {
            let mut variants: Vec<crate::types::FfiVariant> = Vec::new();
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    event = rx.recv() => {
                        match event {
                            Ok(Envelope { event, .. }) => Self::dispatch(
                                &observer,
                                &event,
                                &mut duration_seconds,
                                &mut variants,
                                &state,
                            ),
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    fn u64_to_f64(value: u64) -> Option<f64> {
        let hi = u32::try_from(value >> Self::U64_HIGH_SHIFT).ok()?;
        let lo = u32::try_from(value & u64::from(u32::MAX)).ok()?;
        Some(f64::from(hi).mul_add(Self::U32_MAX_PLUS_ONE, f64::from(lo)))
    }
}

impl Drop for ItemEventBridge {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use kithara::{
        events::{Event, FileError, FileEvent},
        platform::sync::{Arc, Mutex},
    };

    use super::ItemEventBridge;
    use crate::{
        item::{AudioPlayerItem, ItemView},
        observer::ItemObserver,
        types::{FfiError, FfiItemConfig, FfiItemEvent},
    };

    #[derive(Default)]
    struct CollectingItemObserver {
        events: Mutex<Vec<FfiItemEvent>>,
    }

    impl CollectingItemObserver {
        fn take_events(&self) -> Vec<FfiItemEvent> {
            std::mem::take(&mut *self.events.lock())
        }
    }

    impl ItemObserver for CollectingItemObserver {
        fn on_event(&self, event: FfiItemEvent) {
            self.events.lock().push(event);
        }
    }

    fn item_state() -> Arc<Mutex<ItemView>> {
        Arc::clone(
            &AudioPlayerItem::new(FfiItemConfig {
                abr_mode: None,
                audio_id: None,
                headers: None,
                uuid_i64: None,
                url: "https://example.com/quiet-intro.flac".to_string(),
                is_live_stream: false,
                preferred_peak_bitrate: 0.0,
                preferred_peak_bitrate_expensive: 0.0,
            })
            .state,
        )
    }

    fn dispatch_file_error(observer: &Arc<dyn ItemObserver>, state: &Arc<Mutex<ItemView>>) {
        ItemEventBridge::dispatch(
            observer,
            &Event::File(FileEvent::Error {
                error: FileError::Io("boom".into()),
            }),
            &mut None,
            &mut Vec::new(),
            state,
        );
    }

    #[kithara::test]
    fn file_error_maps_to_item_failed() {
        let event = Event::File(FileEvent::Error {
            error: FileError::Io("boom".into()),
        });
        let error = FfiError::try_from(&event).ok();
        assert!(matches!(
            error,
            Some(FfiError::ItemFailed { reason }) if reason == "io: boom"
        ));
    }

    /// The queue settles a failed track too, and reaches the same observer.
    /// Whichever source gets there first owns the pair; a protocol error
    /// arriving after it must not repeat what the item already reported.
    #[kithara::test]
    fn a_protocol_error_after_settlement_does_not_repeat_the_pair() {
        let observer_impl = Arc::new(CollectingItemObserver::default());
        let observer: Arc<dyn ItemObserver> = observer_impl.clone();
        let state = item_state();
        assert!(state.lock().mark_failed(), "the item settles first");

        dispatch_file_error(&observer, &state);

        assert!(
            observer_impl.take_events().is_empty(),
            "a settled item must report its terminal pair once"
        );
    }

    #[kithara::test]
    fn a_protocol_error_on_a_live_item_emits_the_pair() {
        let observer_impl = Arc::new(CollectingItemObserver::default());
        let observer: Arc<dyn ItemObserver> = observer_impl.clone();
        let state = item_state();

        dispatch_file_error(&observer, &state);

        assert_eq!(observer_impl.take_events().len(), 2);
    }
}
