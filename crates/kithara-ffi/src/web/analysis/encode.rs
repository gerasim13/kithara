use std::num::NonZeroU32;

use js_sys::{Float32Array, Float64Array, Object, Reflect};
use kithara::{
    analysis::{BeatArtifact, BeatSnapshot, BeatState, TrackAnalysis, Waveform},
    queue::TrackId,
};
use num_traits::cast;
use wasm_bindgen::JsValue;

use crate::analysis::seconds_at;

/// Scope tag every analysis message carries on the player event channel.
pub(crate) const ANALYSIS_SCOPE: &str = "analysis";

struct Consts;

impl Consts {
    const BANDS_PER_BUCKET: usize = 3;
    const STAGE: usize = 768;
}

/// One analysis publication as the plain JS object the registered callback
/// receives.
pub(crate) fn encode(track_id: TrackId, analysis: &TrackAnalysis) -> Object {
    let rate = analysis.source_sample_rate();
    let beat = analysis.beat();
    let artifact = beat.map(BeatSnapshot::artifact);

    let message = Object::new();
    set(&message, "scope", &JsValue::from_str(ANALYSIS_SCOPE));
    set(&message, "trackId", &number(track_id.as_u64()));
    set(&message, "revision", &number(analysis.revision()));
    set(
        &message,
        "settled",
        &JsValue::from_bool(analysis.is_settled()),
    );
    set(
        &message,
        "sampleRate",
        &JsValue::from_f64(f64::from(rate.get())),
    );
    set(&message, "sourceFrames", &number(analysis.source_frames()));
    set(&message, "waveform", &waveform(analysis).into());
    let beats = artifact.map_or(&[][..], |beat| beat.beats());
    let downbeats = artifact.map_or(&[][..], |beat| beat.downbeats());
    set(&message, "beats", &markers(beats, rate).into());
    set(&message, "downbeats", &markers(downbeats, rate).into());
    set(
        &message,
        "bpm",
        &JsValue::from_f64(artifact.map_or(0.0, BeatArtifact::bpm)),
    );
    set(
        &message,
        "beatFinal",
        &JsValue::from_bool(beat.is_some_and(|snapshot| snapshot.state() == BeatState::Final)),
    );
    message
}

fn waveform(analysis: &TrackAnalysis) -> Float32Array {
    let buckets = analysis.waveform().map_or(&[][..], Waveform::buckets);
    let out = Float32Array::new_with_length(length_of(buckets.len() * Consts::BANDS_PER_BUCKET));
    let mut staged = [0.0_f32; Consts::STAGE];
    let mut written: u32 = 0;
    for group in buckets.chunks(Consts::STAGE / Consts::BANDS_PER_BUCKET) {
        for (slot, bucket) in staged.chunks_exact_mut(Consts::BANDS_PER_BUCKET).zip(group) {
            let [low, mid, high] = slot else { continue };
            *low = bucket.low();
            *mid = bucket.mid();
            *high = bucket.high();
        }
        let filled = group.len() * Consts::BANDS_PER_BUCKET;
        let end = written.saturating_add(length_of(filled));
        out.subarray(written, end).copy_from(&staged[..filled]);
        written = end;
    }
    out
}

fn markers(frames: &[u64], rate: NonZeroU32) -> Float64Array {
    let out = Float64Array::new_with_length(length_of(frames.len()));
    let mut staged = [0.0_f64; Consts::STAGE];
    let mut written: u32 = 0;
    for group in frames.chunks(Consts::STAGE) {
        for (slot, frame) in staged.iter_mut().zip(group) {
            *slot = seconds_at(*frame, rate);
        }
        let end = written.saturating_add(length_of(group.len()));
        out.subarray(written, end).copy_from(&staged[..group.len()]);
        written = end;
    }
    out
}

fn length_of(count: usize) -> u32 {
    cast(count).unwrap_or(u32::MAX)
}

fn number(value: u64) -> JsValue {
    JsValue::from_f64(cast(value).unwrap_or(f64::MAX))
}

fn set(target: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(target, &JsValue::from_str(key), value);
}
