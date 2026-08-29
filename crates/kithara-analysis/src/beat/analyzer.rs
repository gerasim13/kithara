use std::collections::{BTreeMap, BTreeSet};

use bon::Builder;
use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;
use num_traits::cast::ToPrimitive;

use super::{
    detector::{BeatDetectError, BeatDetector, BeatMark, RawBeats},
    grid::{GridParams, build_grid},
    runs::Runs,
};
use crate::{BeatArtifact, analyzer::BeatAnalysisConfig, coverage::FrameRange};

const BUDGET_WINDOWS: usize = 4;

#[derive(Builder)]
pub(crate) struct BeatPassConfig<B>
where
    B: ResamplerBackend,
{
    resampler: BeatAnalysisConfig<B>,
    #[builder(default)]
    params: GridParams,
    sample_pool: SamplePool,
    source_rate: u32,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct BeatAnalyzer<B>
where
    B: ResamplerBackend,
{
    params: GridParams,
    failure: Option<BeatDetectError>,
    sample_pool: SamplePool,
    runs: Runs<B>,
    windows: BTreeMap<usize, RawBeats>,
    short: BTreeSet<usize>,
    hop_frames: usize,
    min_frames: usize,
    ready_frames: usize,
    window_frames: usize,
    #[field(get, copy, vis = "pub(crate)")]
    source_rate: u32,
}

impl<B> BeatAnalyzer<B>
where
    B: ResamplerBackend,
{
    #[must_use]
    pub(crate) fn new(config: BeatPassConfig<B>) -> Self {
        let BeatPassConfig {
            source_rate,
            params,
            resampler: config,
            sample_pool,
        } = config;

        let detector_rate = config.target_rate().max(1);
        let window_frames =
            frames_for_seconds(detector_rate, config.detector_window_seconds().max(1));
        let overlap_seconds = config
            .detector_overlap_seconds()
            .min(config.detector_window_seconds().saturating_sub(1));
        let overlap_frames = frames_for_seconds(detector_rate, overlap_seconds);
        let ready_frames = window_frames.saturating_add(overlap_frames);
        // Four detector windows: detection consumes the front of a run, so the
        // live window plus the runs waiting behind it stay in the budget while
        // a fragmented coverage set cannot grow it with the track length.
        let budget = ready_frames.saturating_mul(BUDGET_WINDOWS);

        Self {
            params,
            hop_frames: window_frames.saturating_sub(overlap_frames).max(1),
            min_frames: frames_for_seconds(detector_rate, config.detector_min_window_seconds())
                .min(window_frames)
                .max(1),
            ready_frames,
            window_frames,
            runs: Runs::new(config, sample_pool.clone(), source_rate, budget),
            windows: BTreeMap::new(),
            short: BTreeSet::new(),
            failure: None,
            sample_pool,
            source_rate,
        }
    }

    pub(crate) fn unanalysed(&self) -> Vec<FrameRange> {
        self.runs
            .dropped()
            .iter()
            .map(|(from, to)| FrameRange::new(*from, to.saturating_sub(*from)))
            .collect()
    }

    pub(crate) fn snapshot(
        &mut self,
        detector: &mut dyn BeatDetector,
        ending: bool,
    ) -> Result<BeatArtifact, BeatDetectError> {
        if let Some(e) = self.failure.take() {
            return Err(e);
        }
        if ending {
            self.runs.flush();
        }
        self.detect(detector, ending)?;

        let mut raw = RawBeats {
            beats: Vec::new(),
            downbeats: Vec::new(),
        };
        for window in self.windows.values() {
            raw.beats.extend_from_slice(&window.beats);
            raw.downbeats.extend_from_slice(&window.downbeats);
        }
        normalize_marks(&mut raw.beats);
        normalize_marks(&mut raw.downbeats);

        build_grid(&raw, self.source_rate, &self.params, &self.sample_pool)
            .map_err(|_| BeatDetectError::Buffer)
    }

    pub(crate) fn push_interleaved(
        &mut self,
        pcm: &[f32],
        channels: usize,
        at: u64,
        detector: &mut dyn BeatDetector,
    ) {
        if channels == 0 || self.failure.is_some() {
            return;
        }
        let frames = pcm.len() / channels;
        if frames == 0 {
            return;
        }

        let inv = 1.0 / channels.to_f32().unwrap_or(1.0);
        let mut mono = self
            .sample_pool
            .get_with(|buffer| buffer.resize(frames, 0.0));
        for (dst, frame) in mono.iter_mut().zip(pcm.chunks_exact(channels)) {
            *dst = frame.iter().sum::<f32>() * inv;
        }
        self.runs.push(&mono[..], at);
        drop(mono);

        self.failure = self.detect(detector, false).err();
    }

    fn detect(
        &mut self,
        detector: &mut dyn BeatDetector,
        trailing: bool,
    ) -> Result<(), BeatDetectError> {
        let Self {
            runs,
            windows,
            short,
            hop_frames,
            min_frames,
            ready_frames,
            window_frames,
            ..
        } = self;
        let rate = runs.target_rate().to_f32().unwrap_or(1.0);

        for (start, mono) in runs.spans() {
            let base = runs.offset_in_run(0, start);
            let mut index = base.div_ceil(*hop_frames);
            loop {
                let span = index.saturating_mul(*hop_frames);
                let Some(offset) = span.checked_sub(base) else {
                    break;
                };
                let Some(available) = mono.len().checked_sub(offset).filter(|left| *left > 0)
                else {
                    break;
                };

                let full = available >= *ready_frames;
                if !full && !trailing && available < *min_frames {
                    break;
                }

                let known = windows.contains_key(&index);
                if !known || (full && short.contains(&index)) {
                    let end = if full {
                        offset.saturating_add(*window_frames)
                    } else {
                        mono.len()
                    };
                    let keep = if full { *hop_frames } else { available };
                    let Some(input) = mono.get(offset..end) else {
                        break;
                    };
                    let raw = detector.detect(input)?;
                    let offset_seconds = span.to_f32().unwrap_or(f32::MAX) / rate;
                    let keep_seconds = keep.to_f32().unwrap_or(f32::MAX) / rate;
                    windows.insert(
                        index,
                        RawBeats {
                            beats: window_marks(raw.beats, offset_seconds, keep_seconds),
                            downbeats: window_marks(raw.downbeats, offset_seconds, keep_seconds),
                        },
                    );
                    if full {
                        short.remove(&index);
                    } else {
                        short.insert(index);
                    }
                }
                if !full {
                    break;
                }
                index = index.saturating_add(1);
            }
        }
        Ok(())
    }
}

fn window_marks(marks: Vec<BeatMark>, offset: f32, keep_until: f32) -> Vec<BeatMark> {
    marks
        .into_iter()
        .filter(|mark| mark.at.is_finite() && mark.at >= 0.0 && mark.at < keep_until)
        .map(|mark| BeatMark {
            at: offset + mark.at,
            ..mark
        })
        .collect()
}

fn frames_for_seconds(sample_rate: u32, seconds: u32) -> usize {
    usize::try_from(u64::from(sample_rate) * u64::from(seconds)).unwrap_or(usize::MAX)
}

fn normalize_marks(marks: &mut Vec<BeatMark>) {
    marks.retain(|mark| mark.at.is_finite() && mark.at >= 0.0);
    marks.sort_by(|a, b| a.at.total_cmp(&b.at));
    marks.dedup_by(|dropped, kept| {
        if dropped.at != kept.at {
            return false;
        }
        kept.confidence = kept.confidence.max(dropped.confidence);
        true
    });
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::SamplePool;
    use kithara_platform::sync::{Arc, Mutex};
    use kithara_resampler::{ResamplerBackend, rubato::RubatoBackend};
    use kithara_test_utils::kithara;
    use num_traits::cast::AsPrimitive;
    use unimock::{MockFn, Unimock, matching};

    use super::{
        super::detector::{BeatDetectError, BeatDetector, BeatDetectorMock, BeatMark, RawBeats},
        BeatAnalyzer, normalize_marks, window_marks,
    };
    use crate::{BeatAnalysisConfig, beat::BeatPassConfig};

    struct Consts;

    impl Consts {
        const SRC: u32 = 44_100;
        const TARGET: usize = 22_050;
    }

    #[kithara::test(native, flash(false))]
    fn a_block_boundary_moves_a_mark_without_touching_its_confidence() {
        let marks = vec![
            BeatMark {
                at: 0.25,
                confidence: 0.9,
            },
            BeatMark {
                at: 0.75,
                confidence: 0.1,
            },
        ];

        let moved = window_marks(marks, 10.0, 1.0);

        assert_eq!(
            moved.iter().map(|mark| mark.at).collect::<Vec<_>>(),
            vec![10.25, 10.75],
            "positions move onto the track timeline"
        );
        assert_eq!(
            moved.iter().map(|mark| mark.confidence).collect::<Vec<_>>(),
            vec![0.9, 0.1],
            "confidences ride along untouched"
        );
    }

    #[kithara::test(native, flash(false))]
    fn two_windows_reporting_one_beat_keep_the_surer_answer() {
        let mut marks = vec![
            BeatMark {
                at: 1.0,
                confidence: 0.4,
            },
            BeatMark {
                at: 1.0,
                confidence: 0.8,
            },
            BeatMark {
                at: 2.0,
                confidence: 0.6,
            },
        ];

        normalize_marks(&mut marks);

        assert_eq!(marks.len(), 2, "the doubled beat is one mark");
        assert_eq!(marks[0].confidence, 0.8, "the surer window wins");
        assert_eq!(marks[1].confidence, 0.6);
    }

    fn empty_raw() -> RawBeats {
        RawBeats {
            beats: Vec::new(),
            downbeats: Vec::new(),
        }
    }

    fn sample_pool() -> SamplePool {
        SamplePool::default()
    }

    fn analyzer(
        source_rate: u32,
        config: BeatAnalysisConfig<RubatoBackend>,
    ) -> BeatAnalyzer<RubatoBackend> {
        BeatAnalyzer::new(
            BeatPassConfig::builder()
                .source_rate(source_rate)
                .resampler(config)
                .sample_pool(sample_pool())
                .build(),
        )
    }

    fn detector(check: impl Fn(&[f32]) -> RawBeats + Send + Sync + 'static) -> Unimock {
        Unimock::new(
            BeatDetectorMock
                .each_call(matching!(_))
                .answers_arc(Arc::new(move |_, mono| Ok(check(mono)))),
        )
    }

    fn stereo(frames: usize, f: impl Fn(usize) -> f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let s = f(n);
            out.push(s);
            out.push(s);
        }
        out
    }

    fn push_chunked<B>(
        analyzer: &mut BeatAnalyzer<B>,
        pcm: &[f32],
        frames_per_chunk: usize,
        detector: &mut dyn BeatDetector,
    ) where
        B: ResamplerBackend,
    {
        let mut at = 0;
        for chunk in pcm.chunks(frames_per_chunk * 2) {
            analyzer.push_interleaved(chunk, 2, at, detector);
            at += u64::try_from(chunk.len() / 2).unwrap_or(0);
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let n: f32 = samples.len().as_();
        (samples.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
    }

    #[kithara::test]
    fn resamples_all_input_without_tail_loss() {
        // 2.0 s of 440 Hz at 44.1 kHz must reach the detector as exactly
        // 2.0 s at 22 050 Hz, with real signal all the way to the end —
        // the resampler tail must be flushed, not dropped.
        let step = std::f32::consts::TAU * 440.0 / 44_100.0;
        let pcm = stereo(2 * 44_100, |n| {
            let t: f32 = n.as_();
            0.5 * (step * t).sin()
        });
        let mut analyzer = analyzer(Consts::SRC, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector = detector(|mono| {
            assert_eq!(
                mono.len(),
                2 * Consts::TARGET,
                "every input frame must reach the detector at 22 050 Hz"
            );
            let whole = rms(mono);
            assert!(
                (whole - 0.354).abs() < 0.05,
                "sine RMS must survive resampling, got {whole}"
            );
            let tail = rms(&mono[mono.len() - 256..]);
            assert!(
                tail > 0.2,
                "the final 256 samples must carry signal (tail flushed), rms {tail}"
            );
            empty_raw()
        });
        push_chunked(&mut analyzer, &pcm, 1000, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
    }

    #[kithara::test]
    fn resampler_delay_is_trimmed_so_positions_stay_aligned() {
        // 1 s silence then 1 s of DC 0.5: the step must sit at output
        // sample ~22050. An untrimmed resampler delay shifts it late.
        let pcm = stereo(2 * 44_100, |n| if n < 44_100 { 0.0 } else { 0.5 });
        let mut analyzer = analyzer(Consts::SRC, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector = detector(|mono| {
            assert_eq!(mono.len(), 2 * Consts::TARGET);
            let crossing = mono
                .iter()
                .position(|s| s.abs() > 0.25)
                .expect("the step must appear in the output");
            let expected = Consts::TARGET;
            assert!(
                crossing.abs_diff(expected) <= 64,
                "step must stay at its source position: got {crossing}, want ~{expected}"
            );
            empty_raw()
        });
        push_chunked(&mut analyzer, &pcm, 4096, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
    }

    #[kithara::test]
    fn downmix_is_channel_mean() {
        // L = +0.8, R = -0.8 cancels to mono silence.
        let mut pcm = Vec::with_capacity(44_100 * 2);
        for _ in 0..44_100 {
            pcm.push(0.8);
            pcm.push(-0.8);
        }
        let mut analyzer = analyzer(Consts::SRC, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector = detector(|mono| {
            assert_eq!(mono.len(), Consts::TARGET);
            let peak = mono.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
            assert!(peak < 0.05, "cancelling stereo must downmix to ~0: {peak}");
            empty_raw()
        });
        analyzer.push_interleaved(&pcm, 2, 0, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
    }

    #[kithara::test]
    fn passthrough_at_detector_rate() {
        // A 22 050 Hz source needs no resampling: the detector sees the input.
        let pcm = stereo(10_000, |_| 0.25);
        let mut analyzer = analyzer(22_050, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector = detector(|mono| {
            assert_eq!(mono, vec![0.25_f32; 10_000].as_slice());
            empty_raw()
        });
        push_chunked(&mut analyzer, &pcm, 999, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
    }

    #[kithara::test]
    fn custom_detector_rate_controls_passthrough_domain() {
        let config = BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .target_rate(Consts::SRC)
            .build();
        let pcm = stereo(4096, |_| 0.25);
        let mut analyzer = analyzer(Consts::SRC, config);
        let mut detector = detector(|mono| {
            assert_eq!(mono, vec![0.25_f32; 4096].as_slice());
            empty_raw()
        });
        analyzer.push_interleaved(&pcm, 2, 0, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
    }

    #[kithara::test]
    fn detector_input_is_bounded_by_configured_window() {
        let config = BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .target_rate(Consts::SRC)
            .detector_window_seconds(1)
            .detector_overlap_seconds(0)
            .build();
        let pcm = stereo(3 * usize::try_from(Consts::SRC).unwrap_or(0), |_| 0.25);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_detector = Arc::clone(&seen);
        let mut detector = detector(move |mono| {
            seen_for_detector.lock().push(mono.len());
            assert!(mono.len() <= usize::try_from(Consts::SRC).unwrap_or(0));
            empty_raw()
        });
        let mut analyzer = analyzer(Consts::SRC, config);

        push_chunked(&mut analyzer, &pcm, 2048, &mut detector);
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");

        let seen = seen.lock().clone();
        assert_eq!(seen.as_slice(), &[44_100, 44_100, 44_100]);
    }

    #[kithara::test]
    fn a_run_at_the_minimum_is_detected_before_the_flush() {
        let config = BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .target_rate(Consts::SRC)
            .detector_window_seconds(2)
            .detector_overlap_seconds(1)
            .build();
        let pcm = stereo(2 * usize::try_from(Consts::SRC).unwrap_or(0), |_| 0.25);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_detector = Arc::clone(&seen);
        let mut detector = detector(move |mono| {
            seen_for_detector.lock().push(mono.len());
            empty_raw()
        });
        let mut analyzer = analyzer(Consts::SRC, config);

        analyzer.push_interleaved(&pcm, 2, 0, &mut detector);
        assert_eq!(
            seen.lock().as_slice(),
            &[2 * 44_100],
            "the run is usable as soon as it reaches the minimum"
        );
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");

        let seen = seen.lock().clone();
        assert_eq!(
            seen.as_slice(),
            &[2 * 44_100],
            "the flush must not re-run a window that cannot grow"
        );
    }

    #[kithara::test]
    fn a_short_run_yields_a_grid_and_is_refined_when_it_fills() {
        let config = BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .target_rate(Consts::SRC)
            .detector_window_seconds(8)
            .detector_overlap_seconds(1)
            .detector_min_window_seconds(2)
            .build();
        let second = usize::try_from(Consts::SRC).unwrap_or(1);
        let pcm = stereo(12 * second, |_| 0.25);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_detector = Arc::clone(&seen);
        let mut detector = detector(move |mono| {
            seen_for_detector.lock().push(mono.len());
            RawBeats {
                beats: vec![BeatMark::at(0.5)],
                downbeats: vec![BeatMark::at(0.5)],
            }
        });
        let mut analyzer = analyzer(Consts::SRC, config);

        // Three seconds: past the minimum, far short of a window.
        analyzer.push_interleaved(&pcm[..3 * second * 2], 2, 0, &mut detector);
        let early = analyzer
            .snapshot(&mut detector, false)
            .expect("a short run still builds a grid");
        assert!(
            !early.beats().is_empty(),
            "one covered piece must already carry markers"
        );
        assert_eq!(
            seen.lock().len(),
            1,
            "the short run is detected once, not once per push"
        );

        // The rest arrives: the window fills and the estimate is replaced.
        analyzer.push_interleaved(
            &pcm[3 * second * 2..],
            2,
            u64::try_from(3 * second).unwrap_or(0),
            &mut detector,
        );
        analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");
        let seen = seen.lock().clone();
        assert!(
            seen.contains(&(8 * second)),
            "a filled window must be re-detected at its full length, saw {seen:?}"
        );
    }

    #[kithara::test]
    fn finalize_builds_grid_in_source_frames() {
        // 9 downbeats every 2.0 s -> 120 bpm, positions converted at the
        // SOURCE rate (48 kHz here), not the detector's 22 050 Hz.
        let raw = RawBeats {
            beats: (0..33)
                .map(|n| {
                    let t: f32 = n.as_();
                    BeatMark::at(t * 0.5)
                })
                .collect(),
            downbeats: (0..9)
                .map(|n| {
                    let t: f32 = n.as_();
                    BeatMark::at(t * 2.0)
                })
                .collect(),
        };
        let mut analyzer = analyzer(48_000, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector = detector(move |_| raw.clone());
        analyzer.push_interleaved(&stereo(17 * 48_000, |_| 0.1), 2, 0, &mut detector);
        let grid = analyzer
            .snapshot(&mut detector, true)
            .expect("mock detects");

        assert!(
            (grid.bpm() - 120.0).abs() < 1e-6,
            "2 s bars are 120 bpm, got {}",
            grid.bpm()
        );
        assert_eq!(grid.downbeats().len(), 9);
        assert_eq!(grid.downbeats()[1], 96_000, "downbeats are source frames");
        assert_eq!(grid.beats()[1], 24_000, "beats are source frames");
        assert!(
            grid.regions().is_empty(),
            "9 downbeats are below the stable window: tempo only"
        );
    }

    #[kithara::test]
    fn detector_failure_propagates() {
        let mut analyzer = analyzer(Consts::SRC, BeatAnalysisConfig::<RubatoBackend>::default());
        let mut detector =
            Unimock::new(BeatDetectorMock.next_call(matching!(_)).answers(&|_, _| {
                Err(BeatDetectError::Detect {
                    reason: "scripted".to_string(),
                })
            }));
        analyzer.push_interleaved(&stereo(4096, |_| 0.1), 2, 0, &mut detector);
        assert!(analyzer.snapshot(&mut detector, true).is_err());
    }

    #[kithara::test]
    fn shuffled_blocks_place_markers_where_ascending_does() {
        // One detector window per second, so a 6 s source yields several
        // windows and the shuffle actually reorders detected spans.
        let config = BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .target_rate(22_050)
            .detector_window_seconds(1)
            .detector_overlap_seconds(0)
            .build();
        // Short enough that the mono budget never reclaims a span before its
        // window completes: marker equality across arrival orders holds below
        // the budget, and the budget's own behaviour is asserted separately.
        let seconds = 3;
        let frames = seconds * usize::try_from(Consts::SRC).unwrap_or(1);
        let step = std::f32::consts::TAU * 220.0 / 44_100.0;
        let pcm = stereo(frames, |n| {
            let t: f32 = n.as_();
            0.5 * (step * t).sin()
        });

        // Each window reports one beat a quarter of the way in, so the marker
        // positions are a pure function of where the window sits.
        let beats = |_: &[f32]| RawBeats {
            beats: vec![BeatMark::at(0.25)],
            downbeats: vec![BeatMark::at(0.25)],
        };

        let block = usize::try_from(Consts::SRC).unwrap_or(1) * 2;
        let blocks: Vec<(u64, &[f32])> = pcm
            .chunks(block)
            .enumerate()
            .map(|(i, part)| (u64::try_from(i).unwrap_or(0) * 44_100, part))
            .collect();

        let run = |order: &[usize]| {
            let mut analyzer = analyzer(Consts::SRC, config.clone());
            let mut detector = detector(beats);
            for index in order {
                let Some((at, part)) = blocks.get(*index) else {
                    continue;
                };
                analyzer.push_interleaved(part, 2, *at, &mut detector);
            }
            analyzer
                .snapshot(&mut detector, true)
                .expect("mock detects")
                .downbeats()
                .to_vec()
        };

        let ascending = run(&[0, 1, 2]);
        let shuffled = run(&[2, 0, 1]);
        assert!(
            !ascending.is_empty(),
            "the ascending pass must find markers"
        );
        assert_eq!(
            ascending.len(),
            shuffled.len(),
            "shuffled ingestion must find the same markers"
        );
        for (want, got) in ascending.iter().zip(shuffled.iter()) {
            assert!(
                want.abs_diff(*got) <= 64,
                "marker must keep its absolute source frame: want {want}, got {got}"
            );
        }
    }
}
