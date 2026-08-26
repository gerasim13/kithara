use std::sync::atomic::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU8};

use arc_swap::ArcSwapOption;
use kithara_platform::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use kithara_stretch::StretchKind;
use portable_atomic::AtomicF32;

use crate::region::RegionPlan;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct EngineControls {
    keylock: AtomicBool,
    backend: AtomicU8,
}

/// Live playback-speed control shared by the caller and the effect chain.
#[derive(Debug)]
#[non_exhaustive]
pub struct StretchControls {
    speed: Arc<AtomicF32>,
    region_plan: ArcSwapOption<RegionPlan>,
    #[cfg(not(target_arch = "wasm32"))]
    engine: EngineControls,
}

impl StretchControls {
    #[must_use]
    pub fn new(speed: f32) -> Arc<Self> {
        Arc::new(Self {
            speed: Arc::new(AtomicF32::new(speed)),
            region_plan: ArcSwapOption::const_empty(),
            #[cfg(not(target_arch = "wasm32"))]
            engine: EngineControls {
                keylock: AtomicBool::new(false),
                backend: AtomicU8::new(u8::from(StretchKind::default())),
            },
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn backend(&self) -> StretchKind {
        StretchKind::from(self.engine.backend.load(Ordering::Relaxed))
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn keylock(&self) -> bool {
        self.engine.keylock.load(Ordering::Relaxed)
    }

    pub fn set_speed(&self, speed: f32) {
        self.speed.store(speed, Ordering::Relaxed);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_backend(&self, backend: StretchKind) {
        self.engine
            .backend
            .store(u8::from(backend), Ordering::Relaxed);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_keylock(&self, on: bool) {
        self.engine.keylock.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed.load(Ordering::Relaxed)
    }

    delegate::delegate! {
        to self.region_plan {
            /// The active region-stretch plan, if any.
            #[must_use]
            #[call(load_full)]
            pub fn region_plan(&self) -> Option<Arc<RegionPlan>>;
            /// Install or clear the region-stretch plan; picked up on the next chunk.
            #[call(store)]
            pub fn set_region_plan(&self, plan: Option<Arc<RegionPlan>>);
        }
    }
}
