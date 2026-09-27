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
const TWELVE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(11);
const PLANE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(FRAMES + 7);

#[kithara::test(native, flash(false))]
fn layout_and_sanitize_never_allocate() {
    let mut restored = vec![vec![0.25_f32; FRAMES]; SIX.get()];
    let stereo = vec![0.0_f32; TWO.get() * FRAMES];
    let mut six = vec![0.0_f32; SIX.get() * FRAMES];
    assert_no_alloc(|| {
        kithara_dsp::deinterleave_variable(&stereo, NonZeroUsize::MIN, &mut restored, 0..FRAMES);
        kithara_dsp::deinterleave_variable(&stereo, TWO, &mut restored, 0..FRAMES);
        kithara_dsp::deinterleave_variable(&six, SIX, &mut restored, 0..FRAMES);
        kithara_dsp::sanitize(&mut six);
    });
}

#[kithara::test(native, flash(false))]
fn channel_major_layout_never_allocates_at_any_channel_count() {
    let mut planar = vec![0.25_f32; TWELVE.get() * PLANE.get()];
    let mut interleaved = vec![0.0_f32; TWELVE.get() * FRAMES];
    assert_no_alloc(|| {
        for channels in [NonZeroUsize::MIN, TWO, SIX, TWELVE] {
            kithara_dsp::interleave_channel_major(
                &planar,
                PLANE,
                3..3 + FRAMES,
                &mut interleaved,
                channels,
            );
            kithara_dsp::deinterleave_channel_major(
                &interleaved,
                channels,
                &mut planar,
                PLANE,
                3..3 + FRAMES,
            );
        }
    });
}
