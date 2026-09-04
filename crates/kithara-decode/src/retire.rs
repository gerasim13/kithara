use kithara_signal::AudioChunk;

/// Where a chunk goes when the caller must not free it.
pub trait ChunkRetire {
    fn retire(&self, chunk: AudioChunk);
}

/// Sink for callers that are free to deallocate.
pub struct DropChunks;

impl ChunkRetire for DropChunks {
    fn retire(&self, _chunk: AudioChunk) {}
}
