use std::{io, net::SocketAddr};

use kithara_bufpool::PoolError;
use kithara_encode::EncodeError;
use kithara_stream::{AudioCodec, ContainerFormat};
use kithara_worker::TaskError;
use thiserror::Error;

/// Errors raised while packaging or serving a live stream.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BroadcastError {
    #[error("HLS broadcast does not support {codec:?} in {container:?}")]
    UnsupportedProfile {
        codec: AudioCodec,
        container: ContainerFormat,
    },

    #[error("live master output requires stereo, got {channels} channels")]
    LiveChannelCount { channels: u16 },

    #[error("broadcast PCM capacity overflow")]
    CapacityOverflow,

    #[error("the composition-owned sample pool rejected broadcast scratch")]
    Pool(#[from] PoolError),

    #[error("ADTS carries no sampling frequency index for {sample_rate} Hz")]
    UnsupportedSampleRate { sample_rate: u32 },

    #[error("ADTS carries no channel configuration for {channels} channels")]
    UnsupportedChannels { channels: u16 },

    #[error("access unit of {payload} bytes exceeds the {limit}-byte ADTS frame payload")]
    PayloadTooLarge { payload: usize, limit: usize },

    #[error("segment duration {duration_ts} does not fit the playlist time base")]
    DurationOutOfRange { duration_ts: u64 },

    #[error("{field} must be > 0")]
    InvalidConfig { field: &'static str },

    #[error(
        "a {window}-segment playlist spans {span_ts} ticks, short of the {minimum_ts} ticks three target durations need"
    )]
    PlaylistTooShort {
        window: usize,
        span_ts: u64,
        minimum_ts: u64,
    },

    #[error("the origin cannot bind {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },

    #[error("the origin cannot start the thread that serves {addr}")]
    Serve {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },

    #[error("the live encoder failed")]
    Encode(#[from] EncodeError),

    #[error("the broadcast worker rejected its task")]
    Worker(#[from] TaskError),
}

/// Result type for live packaging operations.
pub type BroadcastResult<T> = Result<T, BroadcastError>;
