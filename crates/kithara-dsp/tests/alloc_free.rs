#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroUsize;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kithara_dsp::filter::{Biquad, rbj};
use kithara_test_utils::kithara;

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const FRAMES: usize = 1024;
const TWO: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);
const SIX: NonZeroUsize = NonZeroUsize::MIN.saturating_add(5);
const TWELVE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(11);
const PLANE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(FRAMES + 7);

#[kithara::test(native)]
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

#[kithara::test(native)]
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

#[kithara::test(native)]
fn biquad_never_allocates_after_construction() {
    let four = NonZeroUsize::MIN.saturating_add(3);
    let mut filter = Biquad::new(TWO, four).expect("filter builds");
    let mut lookahead = Biquad::new(TWO, four).expect("filter builds");
    let low_pass =
        rbj::low_pass(48_000.0, 4_000.0, std::f64::consts::FRAC_1_SQRT_2).expect("valid low-pass");
    let mut planes = vec![vec![0.25_f32; FRAMES]; TWO.get()];
    assert_no_alloc(|| {
        for section in 0..four.get() {
            filter.retune(section, low_pass).expect("section in range");
            lookahead
                .retune(section, low_pass)
                .expect("section in range");
        }
        filter.settle(&[0.25, 0.25]).expect("one level per channel");
        filter
            .process(&mut planes, 0..FRAMES / 2)
            .expect("shape matches");
        lookahead.copy_state(&filter).expect("same shape");
        lookahead
            .process(&mut planes, FRAMES / 2..FRAMES)
            .expect("shape matches");
        for plane in &mut planes {
            plane.fill(0.0);
        }
        filter
            .process(&mut planes, 0..FRAMES)
            .expect("shape matches");
        for plane in &mut planes {
            plane.fill(0.0);
        }
        filter
            .process(&mut planes, 0..FRAMES)
            .expect("silence resets");
    });
}
