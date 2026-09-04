mod ops;

use std::{array, collections::BTreeMap};

use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use kithara_platform::sync::Arc;
use num_traits::cast::ToPrimitive;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

use super::{
    Band,
    bucket::{Bucket, Waveform},
    bucketize::bucketize,
    params::AnalysisParams,
};
use crate::{
    BlobError,
    blob::Writer,
    coverage::{Coverage, FrameRange},
    progress::{WaveformResume, write_coverage, write_samples},
};

struct Consts;

impl Consts {
    const HANN_A0: f32 = 0.5;
    const HOP_DIVISOR: usize = 4;
    const MAX_PARTIAL: usize = 256;
    const MIN_FFT_SIZE: usize = 2;
}

struct Partial {
    samples: SampleBuffer,
    written: Coverage,
    seq: u64,
}

/// Position-addressed waveform analyzer: mono downmix, then a band-energy
/// series indexed by absolute window position. A window is reduced once its
/// span sits inside one covered run, so ranges may arrive in any order, twice
/// or overlapping. [`Self::snapshot`] folds it into low/mid/high bucket
/// heights.
pub struct WaveformAnalyzer {
    params: AnalysisParams,
    fft: Arc<dyn RealToComplex<f32>>,
    fft_input: SampleBuffer,
    fft_output: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    hann: SampleBuffer,
    downmix: SampleBuffer,
    bands: BTreeMap<u64, [f32; Band::COUNT]>,
    partial: BTreeMap<u64, Partial>,
    band_bin_inv: [f32; Band::COUNT],
    low_mid_bin: usize,
    mid_high_bin: usize,
    opened: u64,
    window_hop: usize,
}

impl WaveformAnalyzer {
    /// Create a waveform analyzer using the registered sample pool.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError`] when the FFT or window buffers do not fit the
    /// shared region budget.
    pub fn new<S>(
        sample_rate: u32,
        params: AnalysisParams,
        pools: &PoolRegion<S>,
    ) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        let fft_size = params.fft_size().max(Consts::MIN_FFT_SIZE);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let fft_input = pools.get_with_len::<f32>(fft_size)?;
        let fft_output = fft.make_output_vec();
        let fft_scratch = fft.make_scratch_vec();

        let hann = hann_window(fft_size, pools)?;
        let bins = fft_output.len();
        let rate = sample_rate.to_f32().unwrap_or(0.0);
        let size_f = fft_size.to_f32().unwrap_or(1.0);
        let bin_hz = if size_f > 0.0 { rate / size_f } else { 0.0 };
        let low_mid_bin = crossover_bin(params.low_mid_hz(), bin_hz, bins);
        let mid_high_bin = crossover_bin(params.mid_high_hz(), bin_hz, bins).max(low_mid_bin);
        // Per-band inverse bin count: divide summed energy by bandwidth so a
        // wide band (mid/high) doesn't outweigh a narrow one (low) by sheer bin
        // count. This makes each band an energy density (RMS-like).
        let inv = |count: usize| 1.0 / count.max(1).to_f32().unwrap_or(1.0);
        let band_bin_inv = [
            inv(low_mid_bin.saturating_sub(1)),
            inv(mid_high_bin.saturating_sub(low_mid_bin)),
            inv(bins.saturating_sub(mid_high_bin)),
        ];

        Ok(Self {
            params,
            fft,
            hann,
            downmix: pools.get::<f32>(),
            low_mid_bin,
            mid_high_bin,
            band_bin_inv,
            fft_input,
            fft_output,
            fft_scratch,
            window_hop: (fft_size / Consts::HOP_DIVISOR).max(1),
            bands: BTreeMap::new(),
            partial: BTreeMap::new(),
            opened: 0,
        })
    }

    /// Fold one interleaved block starting at source frame `at`: downmix to
    /// mono (channel mean), scatter it into every window it touches and reduce
    /// every window the block completes. Blocks may arrive in any order, twice,
    /// or overlapping.
    /// # Errors
    ///
    /// Returns [`PoolError`] when downmix or partial-window storage cannot
    /// grow under the shared region budget.
    pub fn push<S>(
        &mut self,
        pools: &PoolRegion<S>,
        pcm: &[f32],
        channels: usize,
        at: u64,
    ) -> Result<(), PoolError>
    where
        S: HasPool<f32>,
    {
        if channels == 0 {
            return Ok(());
        }
        let frames = pcm.len() / channels;
        let Ok(span) = u64::try_from(frames) else {
            return Ok(());
        };
        if span == 0 {
            return Ok(());
        }

        let inv_channels = 1.0 / channels.to_f32().unwrap_or(1.0);
        self.downmix.ensure_len(frames)?;
        self.downmix.truncate(frames);
        for (dst, frame) in self.downmix.iter_mut().zip(pcm.chunks_exact(channels)) {
            *dst = frame.iter().sum::<f32>() * inv_channels;
        }

        let mono = std::mem::replace(&mut self.downmix, pools.get::<f32>());
        let result = self.push_mono(pools, &mono, at, span);
        self.downmix = mono;
        result
    }

    fn push_mono<S>(
        &mut self,
        pools: &PoolRegion<S>,
        mono: &[f32],
        at: u64,
        span: u64,
    ) -> Result<(), PoolError>
    where
        S: HasPool<f32>,
    {
        let hop = self.hop();
        let size = self.size();
        let end = at.saturating_add(span);
        // Windows overlapping `[at, end)`: `k·hop < end` and `k·hop + size > at`.
        let first = if at >= size { (at - size) / hop + 1 } else { 0 };
        let last = (end - 1) / hop;

        for index in first..=last {
            self.scatter(pools, index, mono, at, end)?;
        }

        for index in first..=last {
            self.reduce_if_complete(index);
        }
        self.evict_overflow();
        Ok(())
    }

    /// Fold the band-energy series into per-bucket band heights, leaving the
    /// pass able to accept further ranges. `extent` is the source length in
    /// frames when it is known: it sets the window count the buckets are
    /// spread over, so bucket boundaries stay put as coverage grows.
    #[must_use]
    pub fn snapshot(&mut self, buckets: usize, extent: Option<u64>) -> Waveform {
        if self.bands.is_empty()
            && let Some(extent) = extent
        {
            self.reduce_padded(extent);
        }

        let total = self.window_count(extent);
        if buckets == 0 || total == 0 {
            return Waveform::default();
        }

        let mut raw = vec![[0.0; Band::COUNT]; total];
        for (&index, energy) in &self.bands {
            if let Ok(index) = usize::try_from(index)
                && let Some(slot) = raw.get_mut(index)
            {
                *slot = *energy;
            }
        }

        let buckets = buckets.min(total);
        let max = |a: [f32; Band::COUNT], b: [f32; Band::COUNT]| array::from_fn(|i| a[i].max(b[i]));
        let energy = bucketize(&raw, buckets, [0.0; Band::COUNT], max);
        let bands = normalize_bands(energy, self.params.band_gain());

        let out: Vec<Bucket> = bands
            .into_iter()
            .map(|b| Bucket::new(b[Band::Low.idx()], b[Band::Mid.idx()], b[Band::High.idx()]))
            .collect();
        Waveform::from(out)
    }

    pub(crate) fn write_resume(&self, out: &mut Vec<u8>) {
        let mut writer = Writer::new(out);
        writer.write_len(self.bands.len());
        for (index, bands) in &self.bands {
            writer.write_u64(*index);
            for band in bands {
                writer.write_f32(*band);
            }
        }
        writer.write_len(self.partial.len());
        for (index, partial) in &self.partial {
            writer.write_u64(*index);
            write_samples(&mut writer, &partial.samples);
            write_coverage(&mut writer, &partial.written);
            writer.write_u64(partial.seq);
        }
        writer.write_u64(self.opened);
    }

    pub(crate) fn restore<S>(
        &mut self,
        pools: &PoolRegion<S>,
        resume: WaveformResume,
    ) -> Result<(), BlobError>
    where
        S: HasPool<f32>,
    {
        if resume.partials.len() > Consts::MAX_PARTIAL {
            return Err(BlobError::Corrupt);
        }

        let mut bands = BTreeMap::new();
        for (index, energy) in resume.bands {
            bands.insert(index, energy);
        }

        let mut partial = BTreeMap::new();
        for held in resume.partials {
            if held.samples.len() != self.window_size()
                || held.seq >= resume.opened
                || bands.contains_key(&held.index)
            {
                return Err(BlobError::Corrupt);
            }
            let span = FrameRange::new(held.index.saturating_mul(self.hop()), self.size());
            if held
                .written
                .runs()
                .iter()
                .any(|range| range.start() < span.start() || range.end() > span.end())
            {
                return Err(BlobError::Corrupt);
            }
            partial.insert(
                held.index,
                Partial {
                    samples: sample_buffer(pools, &held.samples)?,
                    written: held.written,
                    seq: held.seq,
                },
            );
        }

        self.bands = bands;
        self.partial = partial;
        self.opened = resume.opened;
        Ok(())
    }
}

fn sample_buffer<S>(pools: &PoolRegion<S>, samples: &[f32]) -> Result<SampleBuffer, BlobError>
where
    S: HasPool<f32>,
{
    let mut buffer = pools.get_with_len::<f32>(samples.len())?;
    buffer.copy_from_slice(samples);
    Ok(buffer)
}

fn hann_window<S>(size: usize, pools: &PoolRegion<S>) -> Result<SampleBuffer, PoolError>
where
    S: HasPool<f32>,
{
    let mut hann = pools.get_with_len::<f32>(size)?;
    if size <= 1 {
        hann.fill(1.0);
        return Ok(hann);
    }
    let denom = (size - 1).to_f32().unwrap_or(1.0);
    let scale = std::f32::consts::TAU / denom;
    for (n, sample) in hann.iter_mut().enumerate() {
        let phase = scale * n.to_f32().unwrap_or(0.0);
        *sample = Consts::HANN_A0.mul_add(-phase.cos(), Consts::HANN_A0);
    }
    Ok(hann)
}

fn crossover_bin(hz: f32, bin_hz: f32, bins: usize) -> usize {
    if bin_hz <= 0.0 {
        return bins;
    }
    let idx = (hz / bin_hz).to_usize().unwrap_or(bins);
    idx.min(bins)
}

fn normalize_bands(
    energy: Vec<[f32; Band::COUNT]>,
    gain: [f32; Band::COUNT],
) -> Vec<[f32; Band::COUNT]> {
    let mut mags: Vec<[f32; Band::COUNT]> = energy
        .into_iter()
        .map(|e| array::from_fn(|i| e[i].sqrt() * gain[i]))
        .collect();

    let max = mags
        .iter()
        .flat_map(|m| m.iter().copied())
        .fold(0.0_f32, f32::max);
    if max > 0.0 {
        let inv = 1.0 / max;
        for m in &mut mags {
            for v in &mut *m {
                *v *= inv;
            }
        }
    }
    mags
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use num_traits::cast::ToPrimitive;

    use super::WaveformAnalyzer;
    use crate::{
        test_pools::{TestPools, pools},
        waveform::{AnalysisParams, bucket::Bucket},
    };

    struct Consts;

    impl Consts {
        const EPS: f32 = 1e-6;
        const SR: u32 = 44_100;
    }

    struct Pass {
        analyzer: WaveformAnalyzer,
        pools: kithara_bufpool::PoolRegion<TestPools>,
    }

    impl Pass {
        fn new(params: AnalysisParams) -> Self {
            let pools = pools();
            Self {
                analyzer: WaveformAnalyzer::new(Consts::SR, params, &pools)
                    .expect("waveform buffers fit the test region"),
                pools,
            }
        }

        fn push(&mut self, pcm: &[f32], channels: usize, at: u64) {
            self.analyzer
                .push(&self.pools, pcm, channels, at)
                .expect("waveform buffers fit the test region");
        }

        fn whole(&mut self, pcm: &[f32], channels: usize, buckets: usize) -> Vec<Bucket> {
            self.push(pcm, channels, 0);
            let extent = u64::try_from(pcm.len() / channels).unwrap_or(0);
            self.analyzer
                .snapshot(buckets, Some(extent))
                .buckets()
                .to_vec()
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= Consts::EPS
    }

    fn peak(b: &Bucket) -> f32 {
        b.low().max(b.mid()).max(b.high())
    }

    fn flat() -> AnalysisParams {
        AnalysisParams::builder().band_gain([1.0; 3]).build()
    }

    fn sine(freq: f32, samples: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * freq / Consts::SR.to_f32().unwrap_or(1.0);
        (0..samples)
            .map(|n| (step * n.to_f32().unwrap_or(0.0)).sin())
            .collect()
    }

    #[kithara::test]
    fn no_frames_snapshots_empty() {
        assert!(
            Pass::new(AnalysisParams::default())
                .analyzer
                .snapshot(8, None)
                .is_empty()
        );
    }

    #[kithara::test]
    fn zero_buckets_snapshots_empty() {
        let mut pass = Pass::new(AnalysisParams::default());
        assert!(pass.whole(&[0.5; 8192], 1, 0).is_empty());
    }

    #[kithara::test]
    fn loudest_band_normalises_to_one() {
        // Broadband square wave: after shared normalization the single loudest
        // band-bucket reaches exactly 1.0.
        let pcm: Vec<f32> = (0..16_384)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let wave = Pass::new(flat()).whole(&pcm, 1, 10);
        assert_eq!(wave.len(), 10);
        let max = wave.iter().map(peak).fold(0.0_f32, f32::max);
        assert!(
            approx(max, 1.0),
            "loudest band must normalise to 1.0, got {max}"
        );
    }

    #[kithara::test]
    fn silence_is_all_zero() {
        let wave = Pass::new(AnalysisParams::default()).whole(&[0.0; 16_384], 1, 8);
        assert_eq!(wave.len(), 8);
        for b in &wave {
            assert_eq!(*b, Bucket::default(), "silence -> all-zero bucket: {b:?}");
        }
    }

    #[kithara::test]
    fn fewer_frames_than_buckets_stays_finite() {
        let wave = Pass::new(AnalysisParams::default()).whole(&[0.5, -0.5, 0.25], 1, 5);
        assert!(!wave.is_empty() && wave.len() <= 5, "len {}", wave.len());
        for b in &wave {
            for v in [b.low(), b.mid(), b.high()] {
                assert!(
                    v.is_finite() && (0.0..=1.0).contains(&v),
                    "band must stay finite in [0,1]: {b:?}"
                );
            }
        }
    }

    #[kithara::test]
    fn output_is_native_window_resolution_capped() {
        // Ten full FFT windows (4096 + 9 hops of 1024).
        let samples = 4096 + 1024 * 9;
        let pcm = sine(440.0, samples);

        // Above the window count: native resolution, never fabricated.
        assert_eq!(
            Pass::new(flat()).whole(&pcm, 1, 100_000).len(),
            10,
            "large = native count"
        );
        // Below it: still decimates (long-track cap).
        assert_eq!(
            Pass::new(flat()).whole(&pcm, 1, 4).len(),
            4,
            "small request decimates"
        );
    }

    #[kithara::test]
    fn stereo_downmix_is_channel_mean() {
        // L=1, R=-1 cancels to mono 0 -> silence.
        let mut pcm = Vec::with_capacity(16_384 * 2);
        for _ in 0..16_384 {
            pcm.push(1.0);
            pcm.push(-1.0);
        }
        let wave = Pass::new(AnalysisParams::default()).whole(&pcm, 2, 4);
        for b in &wave {
            assert_eq!(*b, Bucket::default(), "cancelling stereo -> silence: {b:?}");
        }
    }

    #[kithara::test]
    fn deterministic_for_same_input() {
        let pcm = sine(440.0, 16_384);
        let run = || Pass::new(AnalysisParams::default()).whole(&pcm, 1, 64);
        assert_eq!(run(), run(), "same PCM must produce the same waveform");
    }

    #[kithara::test]
    fn window_split_across_chunks_matches_unsplit() {
        let pcm = sine(440.0, 16_384);
        let whole = Pass::new(flat()).whole(&pcm, 1, 12);

        // Split at 1500 frames: no boundary lands on a window edge, so every
        // window is assembled from two blocks.
        let mut split = Pass::new(flat());
        for (index, part) in pcm.chunks(1500).enumerate() {
            let at = u64::try_from(index * 1500).unwrap_or(0);
            split.push(part, 1, at);
        }
        let extent = u64::try_from(pcm.len()).unwrap_or(0);
        assert_eq!(
            split.analyzer.snapshot(12, Some(extent)).buckets(),
            whole,
            "a window split across blocks must reduce identically"
        );
    }

    #[kithara::test]
    fn shuffled_and_duplicated_blocks_match_ascending() {
        let pcm = sine(440.0, 16_384);
        let ascending = Pass::new(flat()).whole(&pcm, 1, 12);

        let blocks: Vec<(u64, &[f32])> = pcm
            .chunks(2048)
            .enumerate()
            .map(|(index, part)| (u64::try_from(index * 2048).unwrap_or(0), part))
            .collect();
        let mut shuffled = Pass::new(flat());
        for &(at, part) in [6, 1, 7, 0, 3, 5, 2, 4, 3, 0]
            .iter()
            .filter_map(|i| blocks.get(*i))
        {
            shuffled.push(part, 1, at);
        }
        let extent = u64::try_from(pcm.len()).unwrap_or(0);
        assert_eq!(
            shuffled.analyzer.snapshot(12, Some(extent)).buckets(),
            ascending,
            "shuffled and duplicated blocks must yield the same waveform"
        );
    }

    #[kithara::test]
    fn a_gap_leaves_its_windows_out_until_it_is_filled() {
        let pcm = sine(440.0, 16_384);
        let complete = Pass::new(flat()).whole(&pcm, 1, 12);
        let extent = u64::try_from(pcm.len()).unwrap_or(0);

        let mut gapped = Pass::new(flat());
        gapped.push(&pcm[..4096], 1, 0);
        gapped.push(&pcm[8192..], 1, 8192);
        let partial = gapped.analyzer.snapshot(12, Some(extent));
        assert_ne!(
            partial.buckets(),
            complete,
            "an uncovered span must not carry the covered result"
        );

        gapped.push(&pcm[4096..8192], 1, 4096);
        assert_eq!(
            gapped.analyzer.snapshot(12, Some(extent)).buckets(),
            complete,
            "filling the gap must produce the contiguous result"
        );
    }

    #[kithara::test]
    fn partial_windows_are_capped() {
        // Isolated single-frame blocks complete no window, so each one only
        // opens the windows that contain it.
        let mut pass = Pass::new(flat());
        for block in 0..90_u64 {
            pass.push(&[0.5], 1, block * 100_000);
        }
        assert!(
            pass.analyzer.partial_len() <= 256,
            "live partial windows must stay capped, got {}",
            pass.analyzer.partial_len()
        );
    }

    #[kithara::test]
    fn an_evicted_window_is_not_reduced_from_what_survived() {
        // Half a window arrives, the cap evicts it, then the other half
        // arrives. The window's span is covered, but this analyzer no longer
        // holds the first half: reducing it now would publish a half-silent
        // window instead of leaving the span unanalysed.
        let pcm = sine(440.0, 4096);
        let mut pass = Pass::new(flat());
        pass.push(&pcm[..2048], 1, 0);
        for block in 1..90_u64 {
            pass.push(&[0.5], 1, block * 100_000);
        }
        pass.push(&pcm[2048..], 1, 2048);

        assert!(
            pass.analyzer.reduced(0).is_none(),
            "an evicted window must stay absent, not be reduced from half its samples"
        );
    }

    #[kithara::test]
    fn snapshot_leaves_the_pass_usable() {
        let pcm = sine(440.0, 16_384);
        let extent = u64::try_from(pcm.len()).unwrap_or(0);
        let mut pass = Pass::new(flat());

        pass.push(&pcm[..8192], 1, 0);
        let early = pass.analyzer.snapshot(12, Some(extent));
        pass.push(&pcm[8192..], 1, 8192);
        let late = pass.analyzer.snapshot(12, Some(extent));

        assert_eq!(early.len(), late.len(), "bucket count must not shift");
        assert_ne!(
            early.buckets(),
            late.buckets(),
            "the second snapshot must reflect the added coverage"
        );
        assert_eq!(
            late.buckets(),
            Pass::new(flat()).whole(&pcm, 1, 12),
            "two snapshots must not change the final result"
        );
    }

    fn dominant(freq: f32) -> Bucket {
        // Floor disabled so routing isn't coupled to the gate; unity gain so it
        // isn't coupled to the perceptual balance.
        let params = AnalysisParams::builder()
            .band_gain(flat().band_gain())
            .energy_floor(0.0)
            .build();
        Pass::new(params)
            .whole(&sine(freq, 16_384), 1, 4)
            .into_iter()
            .find(|b| peak(b) > 0.0)
            .unwrap_or_default()
    }

    #[kithara::test]
    fn low_frequency_lands_in_low_band() {
        let b = dominant(80.0);
        assert!(
            b.low() > b.mid() && b.low() > b.high(),
            "80 Hz must be low-dominant: {b:?}"
        );
    }

    #[kithara::test]
    fn mid_frequency_lands_in_mid_band() {
        let b = dominant(1_000.0);
        assert!(
            b.mid() > b.low() && b.mid() > b.high(),
            "1 kHz must be mid-dominant: {b:?}"
        );
    }

    #[kithara::test]
    fn high_frequency_lands_in_high_band() {
        let b = dominant(10_000.0);
        assert!(
            b.high() > b.low() && b.high() > b.mid(),
            "10 kHz must be high-dominant: {b:?}"
        );
    }

    #[kithara::test]
    fn full_spectrum_track_has_no_color_gaps() {
        // Regression for a band series coarser than the bucket count, which left
        // columns with no bar. Every column of a full-spectrum track must carry
        let sr = Consts::SR.to_f32().unwrap_or(1.0);
        let frames = 45 * Consts::SR.to_usize().unwrap_or(0);
        let l = std::f32::consts::TAU * 80.0 / sr;
        let m = std::f32::consts::TAU * 1_000.0 / sr;
        let h = std::f32::consts::TAU * 10_000.0 / sr;
        let pcm: Vec<f32> = (0..frames)
            .map(|n| {
                let t = n.to_f32().unwrap_or(0.0);
                0.3 * ((l * t).sin() + (m * t).sin() + (h * t).sin())
            })
            .collect();
        let wave = Pass::new(AnalysisParams::default()).whole(&pcm, 1, 1500);
        let gaps = wave.iter().filter(|b| peak(b) <= 0.0).count();
        assert_eq!(gaps, 0, "every column must carry a bar");
    }
}
