use kithara_bufpool::PcmPool;
use kithara_decode::PcmSpec;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
use kithara_platform::sync::Arc;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
use crate::musical::{SessionBeat, SourceSchedule};
use crate::{
    tempo::{TempoSlot, TempoSlotError},
    traits::AudioEffect,
};

/// Build `[Tempo?, ..custom]`. Fixed-ratio sample-rate conversion belongs to
/// the decoder plan.
///
/// # Errors
///
/// Returns [`TempoSlotError`] when the configured slot cannot be built on this
/// target.
pub(crate) fn create_effects(
    initial_spec: PcmSpec,
    tempo: Option<&TempoSlot>,
    pool: &PcmPool,
    custom_effects: Vec<Box<dyn AudioEffect>>,
) -> Result<Vec<Box<dyn AudioEffect>>, TempoSlotError> {
    let mut chain: Vec<Box<dyn AudioEffect>> = Vec::new();

    if let Some(tempo) = tempo {
        append_tempo_slot(tempo, &mut chain, initial_spec, pool)?;
    }
    chain.extend(custom_effects);
    Ok(chain)
}

/// A compiled-in backend: the slot is whichever kernel the deck asked for.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
fn append_tempo_slot(
    tempo: &TempoSlot,
    chain: &mut Vec<Box<dyn AudioEffect>>,
    initial_spec: PcmSpec,
    pool: &PcmPool,
) -> Result<(), TempoSlotError> {
    use crate::tempo::streaming::TimeStretchProcessor;

    match tempo {
        TempoSlot::Streaming(controls) => chain.push(Box::new(TimeStretchProcessor::new(
            Arc::clone(controls),
            initial_spec,
            pool.clone(),
        ))),
        TempoSlot::Bound(schedule, session_origin) => chain.push(bound_effect(
            Arc::clone(schedule),
            *session_origin,
            initial_spec,
            pool.clone(),
        )?),
    }
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "stretch-signalsmith"))]
fn bound_effect(
    schedule: Arc<SourceSchedule>,
    session_origin: SessionBeat,
    spec: PcmSpec,
    pool: PcmPool,
) -> Result<Box<dyn AudioEffect>, TempoSlotError> {
    crate::tempo::bound::bound_slot(schedule, session_origin, spec, pool)
        .map_err(|_| TempoSlotError::BoundEngineMissing)
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(feature = "stretch-signalsmith"),
    feature = "stretch-bungee"
))]
fn bound_effect(
    _schedule: Arc<SourceSchedule>,
    _session_origin: SessionBeat,
    _spec: PcmSpec,
    _pool: PcmPool,
) -> Result<Box<dyn AudioEffect>, TempoSlotError> {
    Err(TempoSlotError::BoundEngineMissing)
}

/// No stretch backend compiled in: speed DSP is absent and playback is pinned
/// to unity. An unbound deck degrades to that; a bound deck cannot, because
/// unity is not where its beats belong, so binding is refused.
#[cfg(not(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
)))]
fn append_tempo_slot(
    tempo: &TempoSlot,
    _chain: &mut Vec<Box<dyn AudioEffect>>,
    _initial_spec: PcmSpec,
    _pool: &PcmPool,
) -> Result<(), TempoSlotError> {
    match tempo {
        TempoSlot::Streaming(_) => Ok(()),
        TempoSlot::Bound(_, _) => Err(TempoSlotError::BoundEngineMissing),
    }
}
