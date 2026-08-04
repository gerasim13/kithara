# kithara-apple — Context

Contracts and invariants for the kithara-apple crate; the README is the overview.

`kithara-apple` is the canonical owner of the Apple framework ABI shared by Kithara crates.

## Ownership

- Raw AudioToolbox structs, type aliases, constants, and extern declarations live in `audio_toolbox::sys` (the only publicly re-exported raw surface). Accelerate's raw externs stay private in `accelerate::ffi`.
- Safe RAII wrappers for `AudioConverter`, `AudioFile`, `AudioBufferList`, and POD byte copies (`ApplePod`, `pod_from_prefix`, `pod_to_vec`, `pod_write_to_slice`) live under `audio_toolbox`.
- Foundation and Objective-C binding crates are re-exported through `foundation` (`ns`, `objc`, `block`, `urlsession`). Consumers enable `kithara-apple` features instead of declaring `block2`, `objc2`, or `objc2-foundation` themselves.
- Codec decisions, gapless policy, HTTP semantics, stream read semantics, and resampler algorithms must not move into this crate.

## Feature Surfaces

Each module needs both its feature and an Apple target (`target_os = "macos"` or `"ios"`); on any other target the crate compiles to nothing, so downstream crates need no `cfg` of their own beyond the feature.

- `audio-toolbox` exposes `audio_toolbox`.
- `accelerate` exposes `accelerate`.
- `foundation` exposes the Objective-C/Foundation binding surface needed by Apple platform adapters such as the `NSURLSession` HTTP backend; it is the only feature that pulls optional dependencies.

## Unsafe Boundary

This crate is the canonical unsafe owner for shared Apple framework ABI. Unsafe is confined to the modules that directly bridge Apple C APIs and is justified at the call site. Callers should not need local Apple FFI declarations for shared AudioToolbox, Accelerate, or Foundation dependencies, and downstream crates keep their own unsafe policy strict except for leaf adapter glue that must implement Apple callback protocols.

## Accelerate

Accelerate is an implementation support layer, not a resampler backend. The public helpers expose bounded vector operations — `copy_f32`, `clear_f32`, `ramp_f32`, `linear_interpolate_f32`, `quadratic_interpolate_f32`, and `BiquadFilter`; higher-level crates (currently `kithara-resampler`'s glide engine) decide when those operations are useful.
