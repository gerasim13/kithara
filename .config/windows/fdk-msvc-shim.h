// Forced into every C translation unit the Windows ARM64 lane compiles, since
// a compiler flag for a target reaches every crate built for it. Only MSVC
// needs what is below, and only `fdk-aac-sys` asks for it.
//
// The crate vendors a snapshot of libfdk-aac that predates two upstream fixes.
// One replaced a bare `__attribute__((always_inline))` in
// `libSBRdec/src/hbe.cpp` with the portable `FDK_FORCEINLINE`; the snapshot
// still carries the GCC spelling, and MSVC stops on
// `error C2065: 'always_inline': undeclared identifier`. The attribute only
// asks for inlining, so dropping it costs nothing but a hint. The other is the
// architecture chain in `FDK_archdef.h`, which predates `_M_ARM64` and runs
// off the end into `#warning`, rejected as `fatal error C1021`. Upstream maps
// that macro onto the two below.
//
// Everything is behind `__clang__` because clang needs none of it and is
// actively harmed by it: `aws-lc-sys` selects `clang-cl` for this target, and
// clang declares every NEON type in `arm_neon.h` through
// `__attribute__((neon_vector_type(...)))`. Defining the attribute away turned
// `int8x16_t` into a plain `signed char`, and the vector intrinsics stopped
// type-checking against their own header.
//
// This is a forced include rather than a `/D` because MSVC accepts no
// function-like macro on the command line. Delete the file, and the flag in
// the Windows lane that references it, once `fdk-aac-sys` refreshes what it
// vendors.
#pragma once

#if !defined(__clang__)
#define __attribute__(a)
#define __arm__ 1
#define __ARM_ARCH_8__ 1
#endif
