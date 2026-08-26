use bungee_sys::Request;

use super::{
    buffer::{InputBuffer, PooledPlanar},
    ffi::{NativeOutput, NativeStretcher},
};
use crate::{ElasticConfig, ElasticError};

#[path = "drain.rs"]
mod drain;
#[path = "prime.rs"]
mod prime;
#[path = "render.rs"]
mod render;

#[derive(fieldwork::Fieldwork)]
#[fieldwork(get, copy, vis = "pub(super)")]
pub(super) struct TerminalChunk {
    frames: usize,
    complete: bool,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct StreamCore {
    anchor: Option<f64>,
    cue_grain_pending: bool,
    input: InputBuffer,
    #[field(get, copy, vis = "pub(super)")]
    max_input_frames: usize,
    native: NativeStretcher,
    output: PooledPlanar,
    output_chunk: Option<NativeOutput>,
    output_consumed: usize,
    request: Request,
    request_pending: bool,
    samples_needed: f64,
}

impl StreamCore {
    const INPUT_LOOKAHEAD_DIVISOR: f64 = 2.0;
    const PIPELINE_GRAINS: usize = 4;
    const TERMINAL_GRAIN_LIMIT: usize = 64;

    pub(super) fn new(
        config: &ElasticConfig,
        max_source_frames: usize,
    ) -> Result<Self, ElasticError> {
        let native = NativeStretcher::new(config.sample_rate(), config.channels())?;
        let max_input_frames = native.max_input_frames()?;
        Ok(Self {
            anchor: None,
            cue_grain_pending: false,
            input: InputBuffer::new(config, max_input_frames, max_source_frames)?,
            max_input_frames,
            native,
            output: PooledPlanar::new(config.pool(), config.channels(), max_input_frames)?,
            output_chunk: None,
            output_consumed: 0,
            request: Request {
                position: f64::NAN,
                speed: 1.0,
                pitch: 1.0,
                reset: 0,
            },
            request_pending: false,
            samples_needed: 0.0,
        })
    }

    pub(super) fn prepare_input_capacity(&mut self, capacity: usize) -> Result<(), ElasticError> {
        self.input.prepare_source_capacity(capacity)
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use kithara_bufpool::PcmPool;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{ElasticRequest, backends::bungee::ffi::NativeFault};

    struct Fixture;

    impl Fixture {
        const CHANNELS: usize = 2;
        const CONTEXT_FRAMES: usize = 8192;
        const SAMPLE_RATE: u32 = 48_000;
    }

    fn signal(frames: usize, offset: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let phase = (offset + frame) as f32 * TAU * 440.0 / Fixture::SAMPLE_RATE as f32;
                let sample = phase.sin() * 0.5;
                [sample, sample * -0.5]
            })
            .collect()
    }

    fn anchored_core() -> StreamCore {
        let config = ElasticConfig::builder()
            .pool(PcmPool::default())
            .sample_rate(Fixture::SAMPLE_RATE)
            .channels(Fixture::CHANNELS)
            .max_source_frames(Fixture::CONTEXT_FRAMES)
            .max_output_frames(Fixture::CONTEXT_FRAMES)
            .build()
            .expect("the fixture shape is valid");
        let mut core = StreamCore::new(&config, Fixture::CONTEXT_FRAMES)
            .expect("the anchored fixture core prepares");
        core.prepare_input_capacity(Fixture::CONTEXT_FRAMES * 4)
            .expect("the fixture reserves its complete prime context");
        let history = signal(Fixture::CONTEXT_FRAMES, 0);
        let lookahead = signal(Fixture::CONTEXT_FRAMES, Fixture::CONTEXT_FRAMES);
        let warm_source = signal(Fixture::CONTEXT_FRAMES, Fixture::CONTEXT_FRAMES * 2);
        let mut discarded = vec![0.0; Fixture::CONTEXT_FRAMES * Fixture::CHANNELS];
        core.prime(
            &history,
            &lookahead,
            ElasticRequest::new(Fixture::CONTEXT_FRAMES, Fixture::CONTEXT_FRAMES)
                .expect("the warmup request is valid"),
            &warm_source,
            1.0,
            &mut discarded,
        )
        .expect("the fixture primes at the cue");
        core
    }

    #[kithara::test]
    fn render_call_does_not_prefetch_a_grain_with_stale_controls() {
        const FRAMES: usize = 8192;

        let config = ElasticConfig::builder()
            .pool(PcmPool::default())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(FRAMES)
            .max_output_frames(FRAMES)
            .build()
            .expect("the fixture shape is valid");
        let mut core = StreamCore::new(&config, FRAMES).expect("the fixture core prepares");
        let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
        let source = vec![0.0; FRAMES * config.channels()];
        let mut output = vec![0.0; source.len()];

        core.render(Some(&source), request, 1.0, Some(&mut output))
            .expect("the first quantum renders");

        assert!(
            !core.request_pending,
            "no grain may remain specified with controls from a previous render call"
        );
    }

    #[kithara::test]
    fn adjacent_control_changes_keep_the_original_anchor_and_native_history() {
        const QUANTUM: usize = 64;
        const TRANSITIONS: usize = 32;

        let mut core = anchored_core();
        let mut source_position = Fixture::CONTEXT_FRAMES * 3;
        let mut previous: Option<[f32; Fixture::CHANNELS]> = None;
        for transition in 0..TRANSITIONS {
            let fast = transition.is_multiple_of(2);
            let source_frames = if fast { QUANTUM * 2 } else { QUANTUM };
            let pitch = if fast { 1.5 } else { 1.0 };
            let source = signal(source_frames, source_position);
            let mut output = vec![f32::NAN; QUANTUM * Fixture::CHANNELS];
            core.render(
                Some(&source),
                ElasticRequest::new(source_frames, QUANTUM)
                    .expect("the alternating request is non-empty"),
                pitch,
                Some(&mut output),
            )
            .expect("the adjacent control quantum renders");

            assert_eq!(core.anchor, Some(0.0));
            assert_eq!(core.request.reset, 0);
            assert!(!core.native.is_flushed());
            assert!(output.iter().all(|sample| sample.is_finite()));
            if let Some(previous) = previous {
                for channel in 0..Fixture::CHANNELS {
                    let after = output[channel];
                    assert!(
                        (after - previous[channel]).abs() <= 0.1,
                        "transition {transition} must preserve phase continuity on channel {channel}"
                    );
                }
            }
            previous = Some([
                output[(QUANTUM - 1) * Fixture::CHANNELS],
                output[QUANTUM * Fixture::CHANNELS - 1],
            ]);
            source_position += source_frames;
        }
    }

    #[kithara::test]
    #[case::analyse(NativeFault::Analyse)]
    #[case::synthesise(NativeFault::Synthesise)]
    fn native_failure_clears_the_request_and_remains_reusable(#[case] fault: NativeFault) {
        const QUANTUM: usize = 64;

        let mut core = anchored_core();
        core.native.fail_next(fault);
        let source = signal(QUANTUM, Fixture::CONTEXT_FRAMES * 3);
        let request = ElasticRequest::new(QUANTUM, QUANTUM).expect("unity request");
        let mut output = vec![f32::NAN; QUANTUM * Fixture::CHANNELS];

        assert!(
            core.render(Some(&source), request, 1.0, Some(&mut output))
                .is_err(),
            "the injected native failure must cross the stream boundary"
        );
        assert!(
            !core.request_pending,
            "no failed request may remain pending"
        );
        assert_eq!(core.samples_needed, 0.0);
        assert!(core.output_chunk.is_none());
        assert!(output.iter().all(|sample| sample.is_nan()));

        let recovery_source = signal(
            Fixture::CONTEXT_FRAMES,
            Fixture::CONTEXT_FRAMES * 3 + QUANTUM,
        );
        let mut recovery_output = vec![0.0; Fixture::CONTEXT_FRAMES * Fixture::CHANNELS];
        core.render(
            Some(&recovery_source),
            ElasticRequest::new(Fixture::CONTEXT_FRAMES, Fixture::CONTEXT_FRAMES)
                .expect("the recovery request is valid"),
            1.0,
            Some(&mut recovery_output),
        )
        .expect("the explicitly cleared stream accepts a new request");
    }
}
