/// Explicit backend selection for [`DecoderFactory`](super::DecoderFactory).
///
/// Replaces the legacy boolean `prefer_hardware` flag with a typed
/// enum so callers spell out which backend they want. Failures of the
/// selected backend are terminal — there is no fallback chain.
///
/// Variants are gated on cargo features: a hardware variant exists in
/// the type only when its platform feature is enabled (and only on a
/// matching `target_os`). Picking `DecoderBackend::Apple` on Linux is
/// therefore a compile error, not a runtime `BackendUnavailable`.
///
/// Default = [`DecoderBackend::WebCodecs`] on wasm32 when its feature is
/// enabled. Elsewhere the default is [`DecoderBackend::Symphonia`], unless a
/// device build enables only its platform backend. There is no runtime backend
/// fallback.
///
/// Exactly one backend feature is expected per build: device builds
/// (`apple` / `android`) compile with `--no-default-features` so
/// `symphonia` is absent, and the hardware variant is the sole default.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, derive_more::Display, PartialEq, Eq)]
pub enum DecoderBackend {
    /// Apple `AudioToolbox` (macOS/iOS, requires the `apple` feature).
    #[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
    #[cfg_attr(
        all(
            not(feature = "symphonia"),
            feature = "apple",
            any(target_os = "macos", target_os = "ios")
        ),
        default
    )]
    #[display("apple")]
    Apple,
    /// Android `MediaCodec` (Android, requires the `android` feature).
    #[cfg(all(feature = "android", target_os = "android"))]
    #[cfg_attr(
        all(
            not(feature = "symphonia"),
            feature = "android",
            target_os = "android",
            not(all(feature = "apple", any(target_os = "macos", target_os = "ios")))
        ),
        default
    )]
    #[display("android")]
    Android,
    /// Browser `AudioDecoder` (wasm32, requires the `webcodecs` feature).
    #[cfg(all(target_arch = "wasm32", feature = "webcodecs"))]
    #[cfg_attr(all(target_arch = "wasm32", feature = "webcodecs"), default)]
    #[display("webcodecs")]
    WebCodecs,
    /// Symphonia software decoder (cross-platform, requires the
    /// `symphonia` feature).
    #[cfg(feature = "symphonia")]
    #[cfg_attr(not(all(target_arch = "wasm32", feature = "webcodecs")), default)]
    #[display("symphonia")]
    Symphonia,
}
