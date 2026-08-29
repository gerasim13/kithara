use kithara_signal::AudioChunkInfo;
use kithara_stream::ChunkPosition;

pub(crate) fn chunk_position(info: &AudioChunkInfo) -> ChunkPosition {
    ChunkPosition {
        source_byte_offset: info.source_byte_offset,
        end_position_ns: u64::try_from(info.end_timestamp.as_nanos()).unwrap_or(u64::MAX),
        frame_offset: info.frame_offset,
        frames: u64::from(info.frames),
        source_bytes: info.source_bytes,
    }
}
