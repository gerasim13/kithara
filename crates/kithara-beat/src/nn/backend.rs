/// Runtime for the chunked beat model; mel extraction always uses `RTen` on CPU.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BeatBackend {
    /// `RTen` CPU inference; requires no external inference runtime.
    #[default]
    Cpu,
    /// ONNX Runtime CUDA inference on the selected NVIDIA device.
    ///
    /// Requires a compatible native GPU ONNX Runtime and CUDA/cuDNN libraries.
    /// Provider registration errors are returned, never replaced by CPU inference.
    #[cfg(feature = "cuda")]
    Cuda { device_id: i32 },
}
