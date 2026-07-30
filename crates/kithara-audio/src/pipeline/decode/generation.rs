use kithara_decode::{BlenderProfile, Decoder, GaplessMode, GaplessProfile, PcmChunk};
use kithara_stream::MediaInfo;

use crate::pipeline::gapless::GaplessStage;

pub(crate) struct DecoderGeneration {
    decoder: Box<dyn Decoder>,
    media_info: Option<MediaInfo>,
    base_offset: u64,
    installed_at_seek_epoch: u64,
    gapless_profile: GaplessProfile,
    blender_profile: BlenderProfile,
    gapless: GaplessStage,
}

impl DecoderGeneration {
    pub(crate) fn new(
        decoder: Box<dyn Decoder>,
        media_info: Option<MediaInfo>,
        base_offset: u64,
        installed_at_seek_epoch: u64,
        gapless_mode: GaplessMode,
    ) -> Self {
        let codec = media_info.as_ref().and_then(|info| info.codec);
        let gapless_profile = decoder.gapless_profile(codec);
        let blender_profile = decoder.blender_profile();
        let gapless = GaplessStage::build(gapless_profile, gapless_mode);
        Self {
            decoder,
            media_info,
            base_offset,
            installed_at_seek_epoch,
            gapless_profile,
            blender_profile,
            gapless,
        }
    }

    pub(crate) fn decoder(&self) -> &dyn Decoder {
        self.decoder.as_ref()
    }

    pub(crate) fn decoder_mut(&mut self) -> &mut dyn Decoder {
        self.decoder.as_mut()
    }

    pub(crate) fn media_info(&self) -> Option<&MediaInfo> {
        self.media_info.as_ref()
    }

    pub(crate) fn base_offset(&self) -> u64 {
        self.base_offset
    }

    pub(crate) fn installed_at_seek_epoch(&self) -> u64 {
        self.installed_at_seek_epoch
    }

    pub(crate) fn gapless_profile(&self) -> GaplessProfile {
        self.gapless_profile
    }

    pub(crate) fn blender_profile(&self) -> BlenderProfile {
        self.blender_profile
    }

    pub(crate) fn notify_seek(&mut self) {
        self.gapless.notify_seek();
    }

    pub(crate) fn push(&mut self, chunk: PcmChunk) {
        self.gapless.push(chunk);
    }

    pub(crate) fn next(&mut self) -> Option<PcmChunk> {
        self.gapless.next()
    }

    pub(crate) fn finish(&mut self) {
        self.gapless.set_tail_compensation(self.gapless_profile());
        self.gapless.flush();
    }
}
