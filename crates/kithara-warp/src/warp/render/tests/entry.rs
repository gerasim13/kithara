use std::num::{NonZero, NonZeroU32};

use kithara_signal::AudioChunk;

use super::*;
use crate::{WarpPlan, WarpRenderError, test_grids};

/// Source beat the entered plan activates at: well inside the recording, so
/// the engine history before it is real audio rather than padding.
const CUE_BEAT: f64 = 4.0;

fn entered_plan(host_rate: NonZeroU32) -> WarpPlan {
    let source = test_grids::asset_grid(120.0, spec().sample_rate);
    let target = test_grids::session_grid(100.0, host_rate);
    let output = test_grids::beat_frames(100.0, host_rate) * CUE_BEAT;
    test_grids::plan_over_at(
        source,
        target,
        CUE_BEAT,
        CUE_BEAT,
        SessionFrame::new(num_traits::cast(output).expect("fixture output frame")),
    )
}

fn entered_renderer(plan: WarpPlan, keylock: bool) -> WarpRenderer {
    let controls = StretchControls::new(1.0);
    controls.set_keylock(keylock);
    controls.set_backend(kithara_stretch::StretchKind::Signalsmith);
    let config = WarpConfig::builder().stretch(controls).build();
    let entered = config.entering(Arc::new(plan));
    let mut renderer = Warp::new((), &entered).renderer(spec(), pools());
    renderer.prepare(spec());
    renderer
}

fn source_span(renderer: &WarpRenderer, start: u64, frames: usize) -> AudioChunk {
    let samples: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let value = f32::from(u16::try_from((start as usize + frame) % 97).unwrap_or(0));
            [value / 97.0, -value / 97.0]
        })
        .collect();
    let mut input = chunk(&renderer.pools, &samples);
    input.meta.frame_offset = start;
    input
}

#[kithara::test]
#[cfg(feature = "stretch-signalsmith")]
#[case::keylocked(Consts::SR, true)]
#[case::keylocked_host_rate_differs(48_000, true)]
#[case::varispeed(Consts::SR, false)]
fn an_entered_plan_presents_its_activation_source_after_exact_history(
    #[case] host_rate: u32,
    #[case] keylock: bool,
) {
    let host_rate = NonZeroU32::new(host_rate).expect("fixture host rate");
    let plan = entered_plan(host_rate);
    let activation = plan.activation();
    let revision = activation.revision();
    let cue = activation.source();
    let mut renderer = entered_renderer(plan, keylock);

    let entry = renderer
        .entry_source()
        .expect("an entered renderer names its first source frame");
    assert_eq!(
        entry < cue,
        keylock,
        "only a keylocked engine needs history before the activation"
    );
    let history = usize::try_from(cue - entry).expect("history fits usize");
    if history > 0 {
        let landing = source_span(&renderer, entry, history + 256);
        let preroll = renderer.prepare_quantum(landing.meta, landing.frames());
        let Err(WarpRenderError::Preroll { frames }) = preroll else {
            panic!("audio before the activation is history, got {preroll:?}");
        };
        assert_eq!(frames.get(), history);
        let (head, _) = landing.samples.split_at(history * usize::from(Consts::CH));
        let mut head_meta = landing.meta;
        head_meta.frames = u32::try_from(history).expect("history fits u32");
        renderer
            .admit_preroll(head_meta, head)
            .expect("history continues from the entry source");
    }

    let mut position = cue;
    let output = loop {
        assert!(
            position < cue + 16 * 1024,
            "entered PCM must appear within the engine's warm-up"
        );
        renderer.prepare(spec());
        let at = source_span(&renderer, position, 1024);
        let frames = renderer
            .prepare_quantum(at.meta, at.frames())
            .expect("the entered source continues")
            .get();
        let mut input = source_span(&renderer, position, frames);
        input.meta.frames = u32::try_from(frames).expect("span fits u32");
        position += u64::try_from(frames).expect("span fits u64");
        if let Some(output) = renderer
            .render_quantum(input)
            .continue_value()
            .expect("prepared source shape")
        {
            break output;
        }
    };
    assert_eq!(output.meta.frame_offset, cue);
    assert_eq!(
        output.meta.mapping_revision.map(NonZero::get),
        Some(u64::from(revision)),
        "entered PCM carries the plan's map"
    );
    assert!(output.frames() > 0);
}

#[kithara::test]
#[cfg(feature = "stretch-signalsmith")]
fn an_entered_plan_refuses_a_landing_after_its_activation_source() {
    let plan = entered_plan(spec().sample_rate);
    let cue = plan.activation().source();
    let mut renderer = entered_renderer(plan, true);
    let late = source_span(&renderer, cue + 1, 256);
    let refused = renderer.prepare_quantum(late.meta, late.frames());
    assert!(
        matches!(
            refused,
            Err(WarpRenderError::Engine(
                kithara_stretch::ElasticError::DiscontinuousSource { .. }
            ))
        ),
        "a landing after the cue can never present its first frame, got {refused:?}"
    );
}

#[kithara::test]
fn a_renderer_without_an_entered_plan_names_no_entry_source() {
    let (mut renderer, _) = projection::planned_renderer(StretchControls::new(1.0));
    renderer.prepare(spec());
    assert_eq!(renderer.entry_source(), None);
    let input = source_span(&renderer, 0, 16);
    assert!(matches!(
        renderer.admit_preroll(input.meta, &input.samples),
        Err(WarpRenderError::UnsupportedProjection)
    ));
}
