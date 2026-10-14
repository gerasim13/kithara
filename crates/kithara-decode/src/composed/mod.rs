#[cfg(test)]
use fixtures::{
    counting as test_counting_codec, drain as test_eof_drain_codec, stub as test_stub_codec,
};
#[cfg(test)]
use kithara_platform::time::Duration;
#[cfg(test)]
use kithara_signal::AudioSpec;
#[cfg(all(test, any(feature = "symphonia", android_backend)))]
use kithara_stream::PendingReason;
#[cfg(test)]
use kithara_stream::{AudioCodec, NotReadyCause};
#[cfg(all(test, any(feature = "symphonia", android_backend)))]
use kithara_test_utils::kithara;
pub(crate) use state::{ComposedDecoder, DecoderRuntime};
use state::{HeadStrip, Prefill};
use trim::frames_to_trim;

#[cfg(test)]
use crate::{consts, demuxer::Demuxer, error::DecodeResult, traits::DecoderChunkOutcome};
#[cfg(all(test, any(feature = "symphonia", android_backend)))]
use crate::{
    error::DecodeError,
    traits::{Decoder, DecoderSeekOutcome},
};

mod decoder;
mod state;
mod trim;
#[cfg(test)]
#[macro_use]
mod fixtures;
#[cfg(test)]
#[cfg(any(feature = "symphonia", android_backend))]
mod default_priming_tests;
#[cfg(test)]
mod eof_drain_tests;
#[cfg(test)]
mod hook_tests;
#[cfg(test)]
mod pool_budget_tests;
#[cfg(test)]
mod seek_trim_tests;
#[cfg(test)]
#[cfg(any(feature = "symphonia", android_backend))]
mod smoke_tests;
