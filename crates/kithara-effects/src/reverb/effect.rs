use std::num::{NonZeroU32, NonZeroUsize};

use kithara_bufpool::{HasPool, PoolError, PoolRegion};
use kithara_dsp::{
    add, affine,
    delay::{DelayDiffuser, DelayError, DelayLine, DiffuserError, Tap},
    filter::{FilterError, OnePoleLowPass},
    scale_add, scale_in_place,
};
use num_traits::ToPrimitive;

use super::ReverbConfig;

const COMB_LENGTHS: [u32; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const DIFFUSER_LENGTHS: [u32; 4] = [31, 47, 73, 101];
const COMBS: usize = COMB_LENGTHS.len();
const SCRATCH: usize = 128;
const REFERENCE_RATE: f64 = 44_100.0;
const STEREO_SPREAD: u32 = 13;
const DIFFUSION: f32 = 0.5;

/// Why a reverb could not be prepared or a buffer shape was refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReverbError {
    #[error(transparent)]
    Pool(#[from] PoolError),
    #[error(transparent)]
    Delay(#[from] DelayError),
    #[error(transparent)]
    Diffuser(#[from] DiffuserError),
    #[error(transparent)]
    Filter(#[from] FilterError),
    #[error("reverb section geometry is not representable at {0} Hz")]
    Geometry(NonZeroU32),
    #[error("reverb input and wet output must have the same stereo frame count")]
    Shape,
}

#[derive(Debug)]
struct Comb {
    line: DelayLine,
    tap: Tap,
    feedback: f32,
    input_gain: f32,
}

#[derive(Debug)]
struct Bank {
    combs: Vec<Comb>,
    diffusers: Vec<DelayDiffuser<1>>,
    damping: OnePoleLowPass<COMBS>,
    shortest: NonZeroUsize,
    longest_comb: NonZeroUsize,
}

impl Bank {
    fn new<S: HasPool<f32>>(
        rate: NonZeroU32,
        channel: usize,
        config: ReverbConfig,
        pools: &PoolRegion<S>,
    ) -> Result<Self, ReverbError> {
        let mut shortest = NonZeroUsize::MIN.saturating_add(SCRATCH.saturating_sub(1));
        let mut longest_comb = NonZeroUsize::MIN;
        let mut combs = Vec::with_capacity(COMBS);
        for reference in COMB_LENGTHS {
            let length = frames(rate, reference, channel)?;
            shortest = shortest.min(length);
            longest_comb = longest_comb.max(length);
            let line = DelayLine::new(NonZeroUsize::MIN, length, pools)?;
            let length = length.get().to_f64().ok_or(ReverbError::Geometry(rate))?;
            let tap = line.tap(length)?;
            let feedback = 10.0_f64
                .powf(-3.0 * length / (f64::from(rate.get()) * f64::from(config.decay())))
                .to_f32()
                .ok_or(ReverbError::Geometry(rate))?;
            if !(0.0..1.0).contains(&feedback) {
                return Err(FilterError::Parameters.into());
            }
            let input_gain =
                (1.0 - feedback) / COMBS.to_f32().ok_or(ReverbError::Geometry(rate))?;
            combs.push(Comb {
                line,
                tap,
                feedback,
                input_gain,
            });
        }
        let mut diffusers = Vec::with_capacity(DIFFUSER_LENGTHS.len());
        for reference in DIFFUSER_LENGTHS {
            let length = frames(rate, reference, channel)?;
            shortest = shortest.min(length);
            let line = DelayLine::new(NonZeroUsize::MIN, length, pools)?;
            diffusers.push(DelayDiffuser::new(line, DIFFUSION)?);
        }
        // The normalized control never reaches a frozen damping pole.
        let coefficient = (0.4 * f64::from(config.damping()))
            .to_f32()
            .ok_or(ReverbError::Geometry(rate))?;
        Ok(Self {
            combs,
            diffusers,
            damping: OnePoleLowPass::new(coefficient)?,
            shortest,
            longest_comb,
        })
    }

    fn process_span(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), ReverbError> {
        let len = input.len();
        let mut history = [[0.0_f32; SCRATCH]; COMBS];
        output.fill(0.0);
        for (comb, history) in self.combs.iter().zip(&mut history) {
            let history = history.get_mut(..len).ok_or(ReverbError::Shape)?;
            comb.line.read(0, comb.tap, history)?;
            add(history, output);
        }
        let mut planes = history.each_mut().map(<[f32; SCRATCH]>::as_mut_slice);
        self.damping.process(&mut planes, 0..len)?;
        for (comb, history) in self.combs.iter_mut().zip(&mut history) {
            let history = history.get_mut(..len).ok_or(ReverbError::Shape)?;
            scale_in_place(history, comb.feedback);
            scale_add(input, comb.input_gain, history);
            comb.line.write(&[&*history], 0..len)?;
        }
        for diffuser in &mut self.diffusers {
            diffuser.process(&mut [&mut *output], 0..len)?;
        }
        Ok(())
    }

    fn reset(&mut self) {
        for comb in &mut self.combs {
            comb.line.reset();
        }
        for diffuser in &mut self.diffusers {
            diffuser.reset();
        }
        self.damping.reset();
    }
}

/// Prepared stereo wet network of parallel damped combs and serial diffusers.
#[derive(Debug)]
pub struct Reverb {
    banks: [Bank; 2],
    width: f32,
    span: NonZeroUsize,
    tail_bound: u64,
    silent_frames: u64,
}

impl Reverb {
    /// Prepares the fixed-rate section geometry and coefficients off the audio thread.
    ///
    /// # Errors
    /// Refuses unrepresentable geometry, unavailable pool storage or a failed
    /// DSP design; no partially prepared network is returned.
    pub fn new<S: HasPool<f32>>(
        rate: NonZeroU32,
        config: ReverbConfig,
        pools: &PoolRegion<S>,
    ) -> Result<Self, ReverbError> {
        let left = Bank::new(rate, 0, config, pools)?;
        let right = Bank::new(rate, 1, config, pools)?;
        let span = left.shortest.min(right.shortest);
        let longest = left
            .longest_comb
            .max(right.longest_comb)
            .get()
            .to_u64()
            .ok_or(ReverbError::Geometry(rate))?;
        let tail_bound = (2.0 * f64::from(config.decay()) * f64::from(rate.get()))
            .ceil()
            .to_u64()
            .ok_or(ReverbError::Geometry(rate))?
            .saturating_add(longest);
        Ok(Self {
            banks: [left, right],
            width: f64::from(config.width())
                .to_f32()
                .ok_or(ReverbError::Geometry(rate))?,
            span,
            tail_bound,
            silent_frames: tail_bound,
        })
    }

    /// Writes the stereo wet signal, retaining history across arbitrary partitions.
    ///
    /// # Errors
    /// [`ReverbError::Shape`] when frame counts differ; no signal or state is
    /// changed on shape refusal. DSP errors propagate without a bypass path.
    pub fn process(&mut self, input: [&[f32]; 2], wet: [&mut [f32]; 2]) -> Result<(), ReverbError> {
        let [first, second] = input;
        let frames = first.len();
        if input.iter().any(|plane| plane.len() != frames)
            || wet.iter().any(|plane| plane.len() != frames)
        {
            return Err(ReverbError::Shape);
        }
        let [left, right] = wet;
        let mut cursor = 0;
        while cursor < frames {
            let active = !self.tail_exhausted();
            let mut count = self.span.get().min(frames.wrapping_sub(cursor));
            if active {
                let remaining = self.tail_bound.wrapping_sub(self.silent_frames);
                count = remaining
                    .min(count.to_u64().ok_or(ReverbError::Shape)?)
                    .to_usize()
                    .ok_or(ReverbError::Shape)?;
            }
            let end = cursor.wrapping_add(count);
            let input = [
                first.get(cursor..end).ok_or(ReverbError::Shape)?,
                second.get(cursor..end).ok_or(ReverbError::Shape)?,
            ];
            let mut output = [
                left.get_mut(cursor..end).ok_or(ReverbError::Shape)?,
                right.get_mut(cursor..end).ok_or(ReverbError::Shape)?,
            ];
            let last = input
                .iter()
                .filter_map(|plane| plane.iter().rposition(|sample| *sample != 0.0))
                .max();
            if !active && last.is_none() {
                for plane in output {
                    plane.fill(0.0);
                }
                cursor = end;
                continue;
            }
            for ((input, output), bank) in input
                .into_iter()
                .zip(output.iter_mut())
                .zip(&mut self.banks)
            {
                bank.process_span(input, output)?;
            }
            mix_width(output, self.width, self.span)?;
            let silent = last
                .map_or(count, |frame| count.wrapping_sub(frame).wrapping_sub(1))
                .to_u64()
                .ok_or(ReverbError::Shape)?;
            self.silent_frames = if last.is_some() {
                silent
            } else {
                self.silent_frames.saturating_add(silent)
            };
            if self.tail_exhausted() {
                self.reset();
            }
            cursor = end;
        }
        Ok(())
    }

    /// Returns every section to silence while preserving its configuration.
    pub fn reset(&mut self) {
        for bank in &mut self.banks {
            bank.reset();
        }
        self.silent_frames = self.tail_bound;
    }

    /// Whether the declared finite tail interval has elapsed since the last input.
    #[must_use]
    pub fn tail_exhausted(&self) -> bool {
        self.silent_frames >= self.tail_bound
    }
}

fn frames(rate: NonZeroU32, reference: u32, channel: usize) -> Result<NonZeroUsize, ReverbError> {
    let spread = if channel == 0 { 0 } else { STEREO_SPREAD };
    let reference = reference
        .checked_add(spread)
        .ok_or(ReverbError::Geometry(rate))?;
    (f64::from(reference) * f64::from(rate.get()) / REFERENCE_RATE)
        .round()
        .to_usize()
        .and_then(NonZeroUsize::new)
        .ok_or(ReverbError::Geometry(rate))
}

fn mix_width(wet: [&mut [f32]; 2], width: f32, span: NonZeroUsize) -> Result<(), ReverbError> {
    if width == 1.0 {
        return Ok(());
    }
    let [left, right] = wet;
    let mut mid = [0.0_f32; SCRATCH];
    let toward_mid = 1.0 - width;
    for (left, right) in left
        .chunks_mut(span.get())
        .zip(right.chunks_mut(span.get()))
    {
        let mid = mid.get_mut(..left.len()).ok_or(ReverbError::Shape)?;
        mid.copy_from_slice(left);
        add(right, mid);
        scale_in_place(mid, 0.5);
        if width == 0.0 {
            left.copy_from_slice(mid);
            right.copy_from_slice(mid);
            continue;
        }
        affine(left, width, 0.0);
        scale_add(mid, toward_mid, left);
        affine(right, width, 0.0);
        scale_add(mid, toward_mid, right);
    }
    Ok(())
}
