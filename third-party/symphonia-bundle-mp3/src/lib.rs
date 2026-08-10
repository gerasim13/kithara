// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(rust_2018_idioms)]
#![forbid(unsafe_code)]

mod common;
mod demuxer;
mod header;

pub use demuxer::MpaReader;
#[cfg(any(feature = "mp1", feature = "mp2", feature = "mp3"))]
pub use symphonia_bundle_mp3_upstream::MpaDecoder;
