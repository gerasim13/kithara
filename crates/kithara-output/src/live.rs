use kithara_signal::AudioSpec;

/// Real-time receiver for one planar stereo master block.
///
/// Implementations must not allocate, block, or perform I/O in this call.
pub trait LiveOutput: Send + 'static {
    /// Start accepting master blocks described by `spec`.
    ///
    /// Host calls this on the control thread before the first block in the new
    /// format. Implementations must hand the boundary to their worker without
    /// blocking or performing I/O.
    fn reconfigure(&mut self, spec: AudioSpec);

    /// Receive up to `frames` samples from each planar channel.
    fn write_stereo(&mut self, frames: usize, left: &[f32], right: &[f32]);
}

/// One master-output endpoint that fans each block out to independent outputs.
#[derive(Default)]
pub struct OutputGroup {
    outputs: Vec<Box<dyn LiveOutput>>,
}

impl OutputGroup {
    /// Create an empty group. Outputs are added before the group reaches RT.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            outputs: Vec::new(),
        }
    }

    /// Append one output endpoint.
    pub fn push<O>(&mut self, output: O)
    where
        O: LiveOutput,
    {
        self.outputs.push(Box::new(output));
    }
}

impl LiveOutput for OutputGroup {
    fn reconfigure(&mut self, spec: AudioSpec) {
        for output in &mut self.outputs {
            output.reconfigure(spec);
        }
    }

    fn write_stereo(&mut self, frames: usize, left: &[f32], right: &[f32]) {
        for output in &mut self.outputs {
            output.write_stereo(frames, left, right);
        }
    }
}
