use kithara_dsp::interp::{Interpolation, interpolate};
use kithara_stretch::ElasticError;
use num_traits::ToPrimitive;

use super::renderer_residency::SourceResidency;

pub(super) const SOURCE_RADIUS: u64 = 16;

pub(super) fn source_sample(
    resident: &SourceResidency,
    position: (u128, std::num::NonZeroU128),
    speed: f64,
    terminal: Option<u64>,
    channels: usize,
    channel: usize,
) -> Result<f32, ElasticError> {
    let (numerator, denominator) = position;
    let denominator = denominator.get();
    let source =
        i64::try_from(numerator / denominator).map_err(|_| ElasticError::SampleCountOverflow)?;
    let fraction = (numerator % denominator)
        .to_f64()
        .ok_or(ElasticError::SampleCountOverflow)?
        / denominator
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
    let first = i64::try_from(resident.origin.ok_or(ElasticError::EmptySource)?)
        .map_err(|_| ElasticError::SampleCountOverflow)?;
    let last = terminal
        .map(|end| i64::try_from(end.saturating_sub(1)))
        .transpose()
        .map_err(|_| ElasticError::SampleCountOverflow)?;
    let sample = |offset: i64| {
        let frame = source
            .checked_add(offset)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let frame = last.map_or_else(|| frame.max(first), |last| frame.clamp(first, last));
        let end = u64::try_from(frame)
            .ok()
            .and_then(|frame| frame.checked_add(1))
            .ok_or(ElasticError::SampleCountOverflow)?;
        let range = resident.range(frame, end, channels)?;
        Ok::<_, ElasticError>(resident.samples[range.start + channel])
    };
    if speed <= 1.0 {
        if fraction == 0.0 {
            return sample(0);
        }
        let current = sample(0)?;
        let next = sample(1)?;
        if source == first {
            return Ok((next - current).mul_add(
                fraction.to_f32().ok_or(ElasticError::SampleCountOverflow)?,
                current,
            ));
        }
        let window = [sample(-1)?, current, next];
        let position = [1.0 + fraction.to_f32().ok_or(ElasticError::SampleCountOverflow)?];
        let mut output = [0.0];
        interpolate(Interpolation::Quadratic, &window, &position, &mut output)
            .map_err(|_| ElasticError::EnginePreparation("mapped interpolation failed"))?;
        return Ok(output[0]);
    }
    let radius = i64::try_from(SOURCE_RADIUS).map_err(|_| ElasticError::SampleCountOverflow)?;
    let cutoff = speed.recip();
    let mut total = 0.0;
    let mut weights = 0.0;
    for offset in -radius..=radius {
        let distance = offset.to_f64().ok_or(ElasticError::SampleCountOverflow)? - fraction;
        let angle = std::f64::consts::PI * distance * cutoff;
        let sinc = if angle == 0.0 {
            cutoff
        } else {
            angle.sin() / (std::f64::consts::PI * distance)
        };
        let window = (1.0
            + (std::f64::consts::PI * distance
                / (radius + 1)
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?)
            .cos())
            * 0.5;
        let weight = sinc * window;
        total += f64::from(sample(offset)?) * weight;
        weights += weight;
    }
    (total / weights)
        .to_f32()
        .ok_or(ElasticError::SampleCountOverflow)
}
