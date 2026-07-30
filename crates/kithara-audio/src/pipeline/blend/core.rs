use kithara_decode::{BlenderProfile, PcmChunk};

pub(crate) struct PcmBlender {
    active: BlenderProfile,
}

impl PcmBlender {
    pub(crate) fn new(active: BlenderProfile) -> Self {
        Self { active }
    }

    pub(crate) fn replace_active(&mut self, active: BlenderProfile) {
        self.active = active;
    }

    pub(crate) fn process_active(&self, chunk: PcmChunk) -> PcmChunk {
        debug_assert_eq!(chunk.spec(), self.active.spec());
        chunk
    }
}
