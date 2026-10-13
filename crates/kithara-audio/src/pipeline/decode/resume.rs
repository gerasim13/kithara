use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioChunkInfo};

#[derive(Default)]
pub(crate) struct ResumeCursor {
    decoded: Option<AudioChunkInfo>,
    rendered_source_head: Option<crate::SourceEnd>,
}

impl ResumeCursor {
    pub(crate) fn clear(&mut self) {
        self.decoded = None;
        self.rendered_source_head = None;
    }
    pub(crate) fn commit_source_end(&mut self, end: crate::SourceEnd) {
        self.rendered_source_head = Some(end);
    }
    pub(crate) fn decode_head(&self) -> Option<(u64, u32)> {
        self.decoded.map_or_else(
            || {
                self.rendered_source_head
                    .map(|end| (end.frame(), end.sample_rate().get()))
            },
            |meta| {
                Some((
                    meta.frame_offset.saturating_add(u64::from(meta.frames)),
                    meta.spec.sample_rate.get(),
                ))
            },
        )
    }
    pub(crate) fn record(&mut self, chunk: &AudioChunk) {
        self.decoded = Some(chunk.meta);
    }
    pub(crate) fn decoded_end(&self, source_revision: u64) -> Option<Duration> {
        let meta = self
            .decoded
            .filter(|meta| meta.source_revision == source_revision)?;
        match meta.source_span {
            Some(span) => span.position_at(u64::from(meta.frames)),
            None => meta
                .timestamp
                .checked_add(meta.spec.duration_for(u64::from(meta.frames)).ok()?),
        }
    }
    pub(crate) fn source_end(&self) -> Option<crate::SourceEnd> {
        self.rendered_source_head.or_else(|| {
            self.decoded.map(|meta| {
                meta.source_span.map_or_else(
                    || crate::SourceEnd::new(meta.frame_range().end, meta.spec.sample_rate),
                    |span| {
                        crate::SourceEnd::new(span.end(), span.sample_rate())
                            .with_mapping_revision(span.mapping_revision())
                    },
                )
            })
        })
    }
    pub(crate) fn rebase(&mut self, end: crate::SourceEnd) {
        self.decoded = None;
        self.rendered_source_head = Some(end);
    }
}
