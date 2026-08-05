// Forced into every translation unit of `fdk-aac-sys` on Windows.
//
// The crate vendors a snapshot of libfdk-aac that predates two upstream fixes.
// One of them replaced a bare `__attribute__((always_inline))` in
// `libSBRdec/src/hbe.cpp` with the portable `FDK_FORCEINLINE`; the snapshot
// still carries the GCC spelling, and MSVC stops on
// `error C2065: 'always_inline': undeclared identifier`. The attribute only
// asks for inlining, so dropping it costs nothing but a hint.
//
// This is a forced include rather than a `/D` because MSVC accepts no
// function-like macro on the command line. Delete the file, and the flags in
// the Windows lane that reference it, once `fdk-aac-sys` refreshes what it
// vendors.
#pragma once

#define __attribute__(a)
