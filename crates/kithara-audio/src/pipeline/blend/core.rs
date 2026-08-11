use kithara_decode::{BlenderProfile, PcmChunk};

struct Consts;

impl Consts {
    const HISTORY_FRAMES: usize = 32;
    const JOIN_MICROS: u32 = 20_000;
    /// A stable second-order recurrence has coefficient `2·cos θ`, so `|c| ≤ 2`.
    const MAX_RECURRENCE_COEFF: f32 = 2.0;
    const MICROS_PER_SEC: u32 = 1_000_000;
    const MIN_JOIN_FRAMES: u16 = 2;
    const MIN_JOIN_HISTORY: usize = 3;
    const RECURRENCE_ORDER: usize = 2;
}

enum JoinState {
    Steady,
    Pending,
    /// Ramp counters are `u16` so the per-frame gain is an exact
    /// [`f32::from`] conversion: at 20 ms the ramp is under 4 000 frames even
    /// at 192 kHz, and the saturating conversion pins the pathological case.
    Active {
        frame: u16,
        frames: u16,
    },
}

pub(crate) struct PcmBlender {
    active: BlenderProfile,
    join: JoinState,
    bound: Vec<f32>,
    coefficient: Vec<f32>,
    history: Vec<f32>,
    predicted_current: Vec<f32>,
    predicted_previous: Vec<f32>,
    history_frames: usize,
}

impl PcmBlender {
    pub(crate) fn new(active: BlenderProfile) -> Self {
        let channels = usize::from(active.spec().channels.max(1));
        Self {
            active,
            bound: vec![0.0; channels],
            coefficient: vec![0.0; channels],
            history: vec![0.0; channels.saturating_mul(Consts::HISTORY_FRAMES)],
            history_frames: 0,
            join: JoinState::Steady,
            predicted_current: vec![0.0; channels],
            predicted_previous: vec![0.0; channels],
        }
    }

    fn apply_join(&mut self, chunk: &mut PcmChunk) {
        let JoinState::Active { frame, frames } = &mut self.join else {
            return;
        };
        let channels = usize::from(chunk.spec().channels.max(1));
        for samples in chunk.samples.chunks_exact_mut(channels) {
            if *frame >= *frames {
                self.join = JoinState::Steady;
                return;
            }
            let incoming_gain = f32::from(*frame) / f32::from(*frames);
            let outgoing_gain = 1.0 - incoming_gain;
            for (channel, sample) in samples.iter_mut().enumerate() {
                *sample =
                    self.predicted_current[channel].mul_add(outgoing_gain, *sample * incoming_gain);
                let next = self.coefficient[channel].mul_add(
                    self.predicted_current[channel],
                    -self.predicted_previous[channel],
                );
                self.predicted_previous[channel] = self.predicted_current[channel];
                self.predicted_current[channel] =
                    next.clamp(-self.bound[channel], self.bound[channel]);
            }
            *frame = frame.saturating_add(1);
        }
        if *frame >= *frames {
            self.join = JoinState::Steady;
        }
    }

    fn begin_join(&mut self, chunk: &PcmChunk) {
        if !matches!(self.join, JoinState::Pending) {
            return;
        }
        let channels = usize::from(chunk.spec().channels.max(1));
        if chunk.samples.len() < channels.saturating_mul(Consts::RECURRENCE_ORDER) {
            return;
        }
        for channel in 0..channels {
            let coefficient = recurrence(
                &self.history,
                channels,
                channel,
                Consts::RECURRENCE_ORDER,
                self.history_frames,
            )
            .clamp(-Consts::MAX_RECURRENCE_COEFF, Consts::MAX_RECURRENCE_COEFF);
            let previous =
                self.history[(self.history_frames - Consts::RECURRENCE_ORDER) * channels + channel];
            let current = self.history[(self.history_frames - 1) * channels + channel];
            let observed_bound = self
                .history
                .chunks_exact(channels)
                .take(self.history_frames)
                .map(|samples| samples[channel].abs())
                .chain(
                    chunk
                        .samples
                        .chunks_exact(channels)
                        .map(|samples| samples[channel].abs()),
                )
                .fold(0.0, f32::max);
            let predicted = coefficient.mul_add(current, -previous);
            self.bound[channel] = observed_bound
                .max(predicted.abs())
                .max(recurrence_amplitude(previous, current, coefficient));
            self.coefficient[channel] = coefficient;
            self.predicted_previous[channel] = current;
            self.predicted_current[channel] =
                predicted.clamp(-self.bound[channel], self.bound[channel]);
        }
        let frames = u16::try_from(
            chunk
                .spec()
                .sample_rate
                .get()
                .saturating_mul(Consts::JOIN_MICROS)
                .div_ceil(Consts::MICROS_PER_SEC),
        )
        .unwrap_or(u16::MAX)
        .max(Consts::MIN_JOIN_FRAMES);
        self.join = JoinState::Active { frames, frame: 0 };
    }

    pub(crate) fn join_active(&mut self, active: BlenderProfile) {
        if self.active.spec() == active.spec() && self.history_frames >= Consts::MIN_JOIN_HISTORY {
            self.active = active;
            self.join = JoinState::Pending;
        } else {
            self.replace_active(active);
        }
    }

    pub(crate) fn process_active(&mut self, mut chunk: PcmChunk) -> PcmChunk {
        debug_assert_eq!(chunk.spec(), self.active.spec());
        self.begin_join(&chunk);
        self.apply_join(&mut chunk);
        self.record_tail(&chunk);
        chunk
    }

    fn record_tail(&mut self, chunk: &PcmChunk) {
        let channels = usize::from(chunk.spec().channels.max(1));
        let chunk_frames = chunk.samples.len() / channels;
        if chunk_frames >= Consts::HISTORY_FRAMES {
            let start = (chunk_frames - Consts::HISTORY_FRAMES) * channels;
            self.history.copy_from_slice(&chunk.samples[start..]);
            self.history_frames = Consts::HISTORY_FRAMES;
            return;
        }
        let retained = self
            .history_frames
            .min(Consts::HISTORY_FRAMES - chunk_frames);
        let discarded = self.history_frames.saturating_sub(retained);
        let retained_start = discarded * channels;
        let retained_end = self.history_frames * channels;
        self.history.copy_within(retained_start..retained_end, 0);
        let append_start = retained * channels;
        let append_end = append_start + chunk.samples.len();
        self.history[append_start..append_end].copy_from_slice(&chunk.samples);
        self.history_frames = retained + chunk_frames;
    }

    pub(crate) fn replace_active(&mut self, active: BlenderProfile) {
        self.active = active;
        let channels = usize::from(active.spec().channels.max(1));
        self.bound.resize(channels, 0.0);
        self.coefficient.resize(channels, 0.0);
        self.history
            .resize(channels.saturating_mul(Consts::HISTORY_FRAMES), 0.0);
        self.history_frames = 0;
        self.join = JoinState::Steady;
        self.predicted_current.resize(channels, 0.0);
        self.predicted_previous.resize(channels, 0.0);
    }
}

fn recurrence(history: &[f32], channels: usize, channel: usize, start: usize, end: usize) -> f32 {
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for frame in start..end {
        let antecedent = history[(frame - Consts::RECURRENCE_ORDER) * channels + channel];
        let previous = history[(frame - 1) * channels + channel];
        let current = history[frame * channels + channel];
        numerator = previous.mul_add(current + antecedent, numerator);
        denominator = previous.mul_add(previous, denominator);
    }
    if denominator > f32::EPSILON {
        numerator / denominator
    } else {
        Consts::MAX_RECURRENCE_COEFF
    }
}

fn recurrence_amplitude(previous: f32, current: f32, coefficient: f32) -> f32 {
    let cosine = coefficient / Consts::MAX_RECURRENCE_COEFF;
    let sine_squared = cosine.mul_add(-cosine, 1.0);
    if sine_squared <= f32::EPSILON {
        return 0.0;
    }
    let quadrature = previous.mul_add(-cosine, current);
    previous
        .mul_add(previous, quadrature * quadrature / sine_squared)
        .sqrt()
}
