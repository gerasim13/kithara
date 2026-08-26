use std::cmp::Ordering;

use num_traits::ToPrimitive;

use super::StreamCore;
use crate::{ElasticError, ElasticRequest};

impl StreamCore {
    pub(in super::super) fn render(
        &mut self,
        source: Option<&[f32]>,
        request: ElasticRequest,
        pitch: f64,
        output: Option<&mut [f32]>,
    ) -> Result<(), ElasticError> {
        self.render_inner(source, request, pitch, output, false)
    }

    pub(in super::super) fn probe_silence(
        &mut self,
        request: ElasticRequest,
    ) -> Result<(), ElasticError> {
        self.render_inner(None, request, 1.0, None, true)
    }

    fn render_inner(
        &mut self,
        source: Option<&[f32]>,
        request: ElasticRequest,
        pitch: f64,
        output: Option<&mut [f32]>,
        end_of_input: bool,
    ) -> Result<(), ElasticError> {
        let result = (|| {
            self.input.append(source, request.source_frames())?;
            let (_, _, target) = self.begin_request(request, pitch)?;
            if self.anchor.is_some() {
                self.render_anchored(target, output, end_of_input)
            } else {
                self.render_target(
                    self.input.end(),
                    request,
                    target,
                    output,
                    end_of_input,
                    |_| Ok(()),
                )
            }
        })();
        if let Err(error) = result {
            self.recover()?;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn begin_request(
        &mut self,
        request: ElasticRequest,
        pitch: f64,
    ) -> Result<(f64, f64, usize), ElasticError> {
        let input = request
            .source_frames()
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_count = request
            .output_frames()
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.request.speed = input / output_count;
        self.request.pitch = pitch;
        self.samples_needed += output_count;
        let target = self
            .samples_needed
            .round()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok((input, output_count, target))
    }

    pub(super) fn render_target<F>(
        &mut self,
        input_end: i32,
        request: ElasticRequest,
        target: usize,
        mut output: Option<&mut [f32]>,
        end_of_input: bool,
        mut prepare_input: F,
    ) -> Result<(), ElasticError>
    where
        F: FnMut(&mut Self) -> Result<(), ElasticError>,
    {
        let input = request
            .source_frames()
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_count = request
            .output_frames()
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut rendered = 0;
        while rendered != target {
            rendered += self.consume(target - rendered, output.as_deref_mut(), rendered);
            if rendered == target {
                break;
            }
            let remaining = (target - rendered)
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
            let position = self.position(input_end, input, output_count, remaining)?;
            self.request.reset =
                u8::from(position.partial_cmp(&self.request.position) != Some(Ordering::Greater));
            self.request.position = position;
            self.input.set_requested(self.native.specify(&self.request));
            self.request_pending = true;
            prepare_input(self)?;
            self.synthesise(true, end_of_input)?;
        }
        self.samples_needed -= rendered.to_f64().ok_or(ElasticError::SampleCountOverflow)?;
        Ok(())
    }

    fn render_anchored(
        &mut self,
        target: usize,
        mut output: Option<&mut [f32]>,
        end_of_input: bool,
    ) -> Result<(), ElasticError> {
        let anchor = self.anchor.ok_or(ElasticError::EnginePreparation(
            "Bungee anchored render has no source position",
        ))?;
        let mut rendered = 0;
        while rendered != target {
            self.discard_before(anchor)?;
            rendered += self.consume(target - rendered, output.as_deref_mut(), rendered);
            if rendered == target {
                break;
            }
            self.schedule_anchored(end_of_input)?;
        }
        self.samples_needed -= rendered.to_f64().ok_or(ElasticError::SampleCountOverflow)?;
        Ok(())
    }

    pub(super) fn schedule_anchored(&mut self, end_of_input: bool) -> Result<(), ElasticError> {
        if self.cue_grain_pending {
            self.cue_grain_pending = false;
        } else {
            self.native.next(&mut self.request);
        }
        self.input.set_requested(self.native.specify(&self.request));
        self.request_pending = true;
        self.synthesise(true, end_of_input)
    }

    pub(super) fn position(
        &self,
        input_end: i32,
        input_frames: f64,
        output_frames: f64,
        remaining_output_frames: f64,
    ) -> Result<f64, ElasticError> {
        Ok(f64::from(input_end)
            - self
                .max_input_frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?
                / Self::INPUT_LOOKAHEAD_DIVISOR
            - input_frames * remaining_output_frames / output_frames)
    }

    pub(super) fn synthesise(
        &mut self,
        valid: bool,
        end_of_input: bool,
    ) -> Result<(), ElasticError> {
        if !self.request_pending {
            return Err(ElasticError::EnginePreparation(
                "Bungee synthesis has no specified input grain",
            ));
        }
        self.input.analyse(&mut self.native, valid, end_of_input)?;
        self.request_pending = false;
        self.output_chunk = Some(
            self.native
                .synthesise(&mut self.output.samples, self.output.stride)?,
        );
        self.output_consumed = 0;
        Ok(())
    }

    pub(super) fn consume(
        &mut self,
        wanted: usize,
        output: Option<&mut [f32]>,
        output_frame: usize,
    ) -> usize {
        let Some(chunk) = self.output_chunk.as_ref().filter(|chunk| chunk.valid) else {
            return 0;
        };
        let frames = wanted.min(chunk.frames.saturating_sub(self.output_consumed));
        if let Some(output) = output {
            self.output.copy_interleaved(
                self.output_consumed..self.output_consumed + frames,
                output,
                output_frame,
            );
        }
        self.output_consumed += frames;
        frames
    }
}
