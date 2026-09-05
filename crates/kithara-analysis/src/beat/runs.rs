use std::num::NonZeroU32;

use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use kithara_resampler::{MonoStream, MonoStreamConfig, ResamplerBackend, ResamplerOptions};
use num_traits::cast::ToPrimitive;
use tracing::debug;

use super::detector::BeatDetectError;
use crate::{
    BlobError,
    analyzer::BeatAnalysisConfig,
    blob::Writer,
    progress::{BeatRunResume, write_samples},
};

struct Run {
    start: u64,
    end: u64,
    mono: SampleBuffer,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(super) struct Runs<B>
where
    B: ResamplerBackend,
{
    runs: Vec<Run>,
    config: BeatAnalysisConfig<B>,
    budget: usize,
    #[field(get, vis = "pub(super)")]
    dropped: Vec<(u64, u64)>,
    ratio: f64,
    source_rate: u32,
    #[field(get, copy, vis = "pub(super)")]
    target_rate: u32,
}

impl<B> Runs<B>
where
    B: ResamplerBackend,
{
    pub(super) fn new(config: BeatAnalysisConfig<B>, source_rate: u32, budget: usize) -> Self {
        let target_rate = config.target_rate().max(1);
        let source = f64::from(source_rate.max(1));
        Self {
            runs: Vec::new(),
            ratio: f64::from(target_rate) / source,
            budget,
            dropped: Vec::new(),
            config,
            source_rate: source_rate.max(1),
            target_rate,
        }
    }

    fn held(&self) -> usize {
        self.runs.iter().map(|run| run.mono.len()).sum()
    }

    #[cfg(test)]
    pub(super) fn held_frames(&self) -> usize {
        self.held()
    }

    fn enforce_budget(&mut self) {
        let mut held = self.held();
        while held > self.budget {
            let over = held - self.budget;
            let Some(run) = self.runs.first_mut() else {
                return;
            };
            let drop_detector = over.min(run.mono.len());
            let source = self.ratio.recip() * drop_detector.to_f64().unwrap_or(0.0);
            let source = source
                .round()
                .to_u64()
                .unwrap_or(0)
                .min(run.end - run.start);
            let at = run.start;
            let exact = (source.to_f64().unwrap_or(0.0) * self.ratio)
                .round()
                .to_usize()
                .unwrap_or(0)
                .min(run.mono.len());
            if exact == 0 {
                return;
            }

            run.mono.drain(..exact);
            run.start = at.saturating_add(source);
            held -= exact;
            self.dropped.push((at, run.start));
            debug!(
                from = at,
                to = run.start,
                "beat analysis: detector mono reclaimed; range left unanalysed"
            );
            if run.start >= run.end {
                self.runs.remove(0);
            } else {
                run.mono.shrink_to_fit();
            }
        }
    }

    fn detector_frames(&self, frames: u64) -> usize {
        let frames: f64 = frames.to_f64().unwrap_or(0.0);
        (frames * self.ratio)
            .round()
            .to_usize()
            .unwrap_or(usize::MAX)
    }

    pub(super) fn flush(&mut self) -> Result<(), BeatDetectError> {
        for index in 0..self.runs.len() {
            let Some(span) = self
                .runs
                .get(index)
                .map(|run| run.end.saturating_sub(run.start))
            else {
                continue;
            };
            let expected = self.detector_frames(span);
            let Some(run) = self.runs.get_mut(index) else {
                continue;
            };
            pad(&mut run.mono, expected)?;
        }
        Ok(())
    }

    pub(super) fn write_resume(&self, writer: &mut Writer<'_>) {
        writer.write_len(self.runs.len());
        for run in &self.runs {
            writer.write_u64(run.start);
            writer.write_u64(run.end);
            write_samples(writer, &run.mono);
        }
        writer.write_len(self.dropped.len());
        for (from, to) in &self.dropped {
            writer.write_u64(*from);
            writer.write_u64(*to);
        }
    }

    pub(super) fn restore<S>(
        &mut self,
        pools: &PoolRegion<S>,
        runs: Vec<BeatRunResume>,
        dropped: Vec<(u64, u64)>,
    ) -> Result<(), BlobError>
    where
        S: HasPool<f32>,
    {
        let mut restored: Vec<Run> = Vec::with_capacity(runs.len());
        for run in runs {
            let expected = self.detector_frames(run.end.saturating_sub(run.start));
            if run.mono.len() != expected {
                return Err(BlobError::Corrupt);
            }
            let samples = run.mono.into_vec();
            let mut mono = pools
                .get_with_len::<f32>(samples.len())
                .map_err(|_| BlobError::Corrupt)?;
            mono.copy_from_slice(&samples);
            mono.shrink_to_fit();
            restored.push(Run {
                start: run.start,
                end: run.end,
                mono,
            });
        }
        if restored.iter().any(|run| {
            dropped
                .iter()
                .any(|(from, to)| *from < run.end && run.start < *to)
        }) {
            return Err(BlobError::Corrupt);
        }

        self.runs = restored;
        self.dropped = dropped;
        if self.held() > self.budget {
            return Err(BlobError::Corrupt);
        }
        Ok(())
    }

    pub(super) fn push<S>(
        &mut self,
        pools: &PoolRegion<S>,
        mono: &[f32],
        at: u64,
    ) -> Result<(), BeatDetectError>
    where
        S: HasPool<f32>,
    {
        let Ok(span) = u64::try_from(mono.len()) else {
            return Ok(());
        };
        if span == 0 {
            return Ok(());
        }
        let end = at.saturating_add(span);

        let first = self.runs.partition_point(|run| run.end < at);
        let last = self.runs.partition_point(|run| run.start <= end);
        if first == last {
            let run = self.open(pools, mono, at, end)?;
            self.runs.insert(first, run);
            self.enforce_budget();
            return Ok(());
        }

        let absorbed: Vec<Run> = self.runs.splice(first..last, []).collect();
        if let Some(merged) = self.merge(pools, absorbed, mono, at, end)? {
            self.runs.insert(first, merged);
        }
        self.enforce_budget();
        Ok(())
    }

    pub(super) fn spans(&self) -> impl Iterator<Item = (u64, &[f32])> {
        self.runs.iter().map(|run| (run.start, &run.mono[..]))
    }

    pub(super) fn offset_in_run(&self, start: u64, frame: u64) -> usize {
        self.detector_frames(frame.saturating_sub(start))
    }

    fn merge<S>(
        &mut self,
        pools: &PoolRegion<S>,
        absorbed: Vec<Run>,
        mono: &[f32],
        at: u64,
        end: u64,
    ) -> Result<Option<Run>, BeatDetectError>
    where
        S: HasPool<f32>,
    {
        let base = absorbed.first().map_or(at, |run| run.start.min(at));
        let mut out = pools.get::<f32>();
        let mut cursor = base;

        for run in absorbed {
            if cursor < run.start {
                let Some(piece) = slice(mono, at, cursor, run.start) else {
                    return Ok(None);
                };
                self.segment(pools, &mut out, piece)?;
                pad(
                    &mut out,
                    self.detector_frames(run.start.saturating_sub(base)),
                )?;
            }
            if run.end <= cursor {
                continue;
            }
            let skip = self.detector_frames(cursor.saturating_sub(run.start));
            append(&mut out, run.mono.get(skip..).unwrap_or_default())?;
            cursor = run.end;
            pad(&mut out, self.detector_frames(cursor.saturating_sub(base)))?;
        }

        if cursor < end {
            let Some(piece) = slice(mono, at, cursor, end) else {
                return Ok(None);
            };
            self.segment(pools, &mut out, piece)?;
            cursor = end;
            pad(&mut out, self.detector_frames(cursor.saturating_sub(base)))?;
        }

        Ok(Some(Run {
            start: base,
            end: cursor,
            mono: out,
        }))
    }

    fn open<S>(
        &self,
        pools: &PoolRegion<S>,
        mono: &[f32],
        at: u64,
        end: u64,
    ) -> Result<Run, BeatDetectError>
    where
        S: HasPool<f32>,
    {
        let mut out = pools.get::<f32>();
        self.segment(pools, &mut out, mono)?;
        Ok(Run {
            start: at,
            end,
            mono: out,
        })
    }

    fn segment<S>(
        &self,
        pools: &PoolRegion<S>,
        out: &mut SampleBuffer,
        mono: &[f32],
    ) -> Result<(), BeatDetectError>
    where
        S: HasPool<f32>,
    {
        if self.source_rate == self.target_rate {
            append(out, mono)?;
            return Ok(());
        }
        let mut stream = self.stream(pools)?;
        push_stream(&mut stream, mono, out)?;
        finish_stream(stream, out)
    }

    fn stream<S>(&self, pools: &PoolRegion<S>) -> Result<MonoStream<B>, BeatDetectError>
    where
        S: HasPool<f32>,
    {
        let source_sample_rate = NonZeroU32::new(self.source_rate).unwrap_or(NonZeroU32::MIN);
        let target_sample_rate = NonZeroU32::new(self.target_rate).unwrap_or(NonZeroU32::MIN);
        let config = MonoStreamConfig::builder()
            .backend(self.config.resampler_backend().clone())
            .source_sample_rate(source_sample_rate)
            .target_sample_rate(target_sample_rate)
            .quality(self.config.resampler_quality())
            .options(
                ResamplerOptions::builder()
                    .chunk_size(self.config.block_frames())
                    .build(),
            )
            .pools(pools.clone())
            .build();
        MonoStream::new(config).map_err(resample_error)
    }
}

fn pad(out: &mut SampleBuffer, expected: usize) -> Result<(), PoolError> {
    if out.len() > expected {
        out.truncate(expected);
    } else {
        out.ensure_len(expected)?;
    }
    Ok(())
}

fn append(out: &mut SampleBuffer, src: &[f32]) -> Result<(), PoolError> {
    out.try_extend_from_slice(src)
}

fn push_stream<B>(
    stream: &mut MonoStream<B>,
    mono: &[f32],
    out: &mut SampleBuffer,
) -> Result<(), BeatDetectError>
where
    B: ResamplerBackend,
{
    let mut buffer_error = None;
    let result = stream.push(mono.iter().copied(), |samples| {
        if buffer_error.is_none() {
            buffer_error = append(out, samples).err();
        }
    });
    if let Some(error) = buffer_error {
        return Err(error.into());
    }
    result.map_err(resample_error)
}

fn finish_stream<B>(stream: MonoStream<B>, out: &mut SampleBuffer) -> Result<(), BeatDetectError>
where
    B: ResamplerBackend,
{
    let mut buffer_error = None;
    let result = stream.finish(|samples| {
        if buffer_error.is_none() {
            buffer_error = append(out, samples).err();
        }
    });
    if let Some(error) = buffer_error {
        return Err(error.into());
    }
    result.map_err(resample_error)
}

fn resample_error(error: impl std::fmt::Display) -> BeatDetectError {
    BeatDetectError::Resample {
        reason: error.to_string(),
    }
}

fn slice(mono: &[f32], at: u64, from: u64, to: u64) -> Option<&[f32]> {
    let start = usize::try_from(from.saturating_sub(at)).ok()?;
    let end = usize::try_from(to.saturating_sub(at)).ok()?;
    mono.get(start..end)
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::PoolConfig;
    use kithara_resampler::rubato::RubatoBackend;
    use kithara_test_utils::kithara;

    use super::Runs;
    use crate::{
        BeatAnalysisConfig,
        test_pools::{TestPools, pools, pools_with},
    };

    const SRC: u32 = 44_100;

    struct TestRuns {
        inner: Runs<RubatoBackend>,
        pools: kithara_bufpool::PoolRegion<TestPools>,
    }

    impl TestRuns {
        fn push(&mut self, mono: &[f32], at: u64) {
            self.inner
                .push(&self.pools, mono, at)
                .expect("run buffers fit the test region");
        }

        fn flush(&mut self) {
            self.inner.flush().expect("run buffers fit the test region");
        }
    }

    impl std::ops::Deref for TestRuns {
        type Target = Runs<RubatoBackend>;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    fn runs(source_rate: u32) -> TestRuns {
        budgeted(source_rate, usize::MAX)
    }

    fn budgeted(source_rate: u32, budget: usize) -> TestRuns {
        budgeted_with_pools(source_rate, budget, pools())
    }

    fn budgeted_with_pools(
        source_rate: u32,
        budget: usize,
        pools: kithara_bufpool::PoolRegion<TestPools>,
    ) -> TestRuns {
        TestRuns {
            inner: Runs::new(
                BeatAnalysisConfig::<RubatoBackend>::default(),
                source_rate,
                budget,
            ),
            pools,
        }
    }

    fn non_retaining_pools(max_bytes: usize) -> kithara_bufpool::PoolRegion<TestPools> {
        pools_with(
            max_bytes,
            PoolConfig::builder().max_buffers(32).build(),
            PoolConfig::builder()
                .max_buffers(8)
                .max_retained_capacity(1)
                .build(),
        )
    }

    fn ramp(frames: usize, from: u64) -> Vec<f32> {
        (0..frames)
            .map(|n| {
                let t = (from + n as u64) as f32 / 1000.0;
                t.sin()
            })
            .collect()
    }

    fn layout(runs: &Runs<RubatoBackend>) -> Vec<(u64, usize)> {
        runs.spans()
            .map(|(start, mono)| (start, mono.len()))
            .collect()
    }

    #[kithara::test]
    fn adjacent_blocks_form_one_run() {
        let mut set = runs(SRC);
        set.push(&ramp(4410, 0), 0);
        set.push(&ramp(4410, 4410), 4410);
        set.flush();
        assert_eq!(layout(&set), vec![(0, 4410)], "8820 source frames at 2:1");
    }

    #[kithara::test]
    fn a_gap_keeps_two_runs_until_it_is_filled() {
        let mut set = runs(SRC);
        set.push(&ramp(4410, 0), 0);
        set.push(&ramp(4410, 88_200), 88_200);
        set.flush();
        assert_eq!(layout(&set), vec![(0, 2205), (88_200, 2205)]);

        set.push(&ramp(83_790, 4410), 4410);
        set.flush();
        assert_eq!(
            layout(&set),
            vec![(0, 46_305)],
            "filling the gap joins the runs and pins the total length"
        );
    }

    #[kithara::test]
    fn a_block_before_a_run_extends_it_backwards() {
        let mut set = runs(SRC);
        set.push(&ramp(4410, 4410), 4410);
        set.push(&ramp(4410, 0), 0);
        set.flush();
        assert_eq!(layout(&set), vec![(0, 4410)]);
    }

    #[kithara::test]
    fn shuffled_blocks_land_at_the_same_detector_offsets() {
        let blocks: Vec<(u64, Vec<f32>)> = (0..8u64)
            .map(|i| (i * 4410, ramp(4410, i * 4410)))
            .collect();

        let mut ascending = runs(SRC);
        for (at, pcm) in &blocks {
            ascending.push(pcm, *at);
        }
        ascending.flush();

        let mut shuffled = runs(SRC);
        for index in [5usize, 0, 7, 2, 1, 6, 3, 4] {
            let Some((at, pcm)) = blocks.get(index) else {
                continue;
            };
            shuffled.push(pcm, *at);
        }
        shuffled.flush();

        assert_eq!(layout(&ascending), layout(&shuffled));
        let (_, want) = ascending.spans().next().expect("one run");
        let (_, got) = shuffled.spans().next().expect("one run");
        let drift = want
            .iter()
            .zip(got.iter())
            .filter(|(a, b)| (*a - *b).abs() > 1e-3)
            .count();
        assert!(
            drift * 200 < want.len(),
            "shuffled assembly must track the ascending one, {drift} of {} samples differ",
            want.len()
        );
    }

    #[kithara::test]
    fn the_budget_reclaims_the_earliest_mono_and_reports_it() {
        let mut set = budgeted(SRC, 20_000);
        for block in 0..10u64 {
            set.push(&ramp(4410, block * 4410), block * 4410);
        }
        assert!(
            set.held_frames() <= 20_000,
            "held detector frames must stay under the budget, got {}",
            set.held_frames()
        );

        let dropped = set.dropped();
        assert!(!dropped.is_empty(), "the reclaimed ranges must be reported");
        let (from, _) = dropped.first().copied().unwrap_or((1, 1));
        assert_eq!(from, 0, "the earliest source frames go first");
        let reclaimed: u64 = dropped.iter().map(|(from, to)| to - from).sum();
        assert!(
            reclaimed > 0 && reclaimed < 44_100,
            "the budget reclaims the overflow, not the track: {reclaimed}"
        );
    }

    #[kithara::test]
    fn reclaimed_mono_releases_charged_capacity() {
        const BUDGET: usize = 4096;
        const MAX_CHARGED_BYTES: usize = BUDGET * size_of::<f32>();
        const TARGET_RATE: u32 = 22_050;

        let pools = non_retaining_pools(4 * MAX_CHARGED_BYTES);
        let mut set = budgeted_with_pools(TARGET_RATE, BUDGET, pools.clone());
        for cycle in 0..4u64 {
            let at = cycle * BUDGET as u64;
            set.push(&ramp(BUDGET, at), at);

            assert_eq!(set.held_frames(), BUDGET);
            assert!(
                pools.stats().allocated_bytes <= MAX_CHARGED_BYTES,
                "cycle {cycle} retains {} charged bytes for a {MAX_CHARGED_BYTES}-byte mono budget",
                pools.stats().allocated_bytes
            );
        }
    }

    #[kithara::test]
    fn fragmented_runs_share_the_region_with_loader_scratch() {
        const LOADER_BYTES: usize = 512 * 1024;
        const POOL_BYTES: usize = 2 * LOADER_BYTES;

        let pools = non_retaining_pools(POOL_BYTES);
        let loader = pools
            .get_with_len::<u8>(LOADER_BYTES)
            .expect("initial loader scratch must fit");

        let mut set = budgeted_with_pools(SRC, usize::MAX, pools.clone());
        for fragment in 0..24u16 {
            set.push(&[f32::from(fragment)], u64::from(fragment) * 2);
        }
        assert_eq!(set.spans().count(), 24, "source gaps must remain distinct");
        assert_eq!(loader.len(), LOADER_BYTES);

        drop(loader);
        let loader = pools
            .get_with_len::<u8>(LOADER_BYTES)
            .expect("fragmented analysis must leave capacity for loader scratch");
        assert_eq!(loader.len(), LOADER_BYTES);

        drop(set);
        drop(loader);
        pools
            .get_with_len::<u8>(POOL_BYTES)
            .expect("completed analysis must return its capacity to the region");
    }

    #[kithara::test]
    fn a_non_integer_ratio_keeps_joins_on_position() {
        // 48 kHz -> 22.05 kHz is not a whole ratio, so a per-segment rounding
        // error would show up as a length drift at every join.
        let mut set = runs(48_000);
        for block in 0..10u64 {
            set.push(&ramp(4801, block * 4801), block * 4801);
        }
        set.flush();
        let total = set.offset_in_run(0, 48_010);
        assert_eq!(layout(&set), vec![(0, total)]);
    }
}
