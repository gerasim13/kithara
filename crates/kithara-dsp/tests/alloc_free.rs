#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroUsize;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kithara_test_utils::kithara;

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const FRAMES: usize = 1024;
const TWO: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);
const SIX: NonZeroUsize = NonZeroUsize::MIN.saturating_add(5);

#[kithara::test(native, flash(false))]
fn layout_and_sanitize_never_allocate() {
    let planes = vec![vec![0.25_f32; FRAMES]; SIX.get()];
    let mut restored = planes.clone();
    let mut stereo = vec![0.0_f32; TWO.get() * FRAMES];
    let mut six = vec![0.0_f32; SIX.get() * FRAMES];
    assert_no_alloc(|| {
        kithara_dsp::interleave_variable(&planes[..1], 0..FRAMES, &mut stereo, NonZeroUsize::MIN);
        kithara_dsp::interleave_variable(&planes, 0..FRAMES, &mut stereo, TWO);
        kithara_dsp::interleave_variable(&planes, 0..FRAMES, &mut six, SIX);
        kithara_dsp::deinterleave_variable(&stereo, NonZeroUsize::MIN, &mut restored, 0..FRAMES);
        kithara_dsp::deinterleave_variable(&stereo, TWO, &mut restored, 0..FRAMES);
        kithara_dsp::deinterleave_variable(&six, SIX, &mut restored, 0..FRAMES);
        kithara_dsp::sanitize(&mut six);
    });
}
