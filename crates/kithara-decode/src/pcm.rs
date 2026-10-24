use kithara_bufpool::SampleBuffer;
use kithara_signal::AudioSpec;
use symphonia_codec_pcm::PcmDecoder;
use symphonia_core::{
    codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions},
    packet::PacketRef,
    units::{Duration, Timestamp},
};

use crate::{
    DecodeError, DecodeResult,
    codec::{DecodedPacket, FrameCodec},
    demuxer::{Frame, PacketMetadata},
    types::checked_audio_spec,
};

pub(crate) struct PcmCodec {
    inner: PcmDecoder,
    spec: AudioSpec,
}

impl PcmCodec {
    pub(crate) fn open(params: &AudioCodecParameters) -> DecodeResult<Self> {
        let inner = PcmDecoder::try_new(params, &AudioDecoderOptions::default())
            .map_err(DecodeError::backend)?;
        let decoded = inner.last_decoded();
        let native = decoded.spec();
        let spec = checked_audio_spec(
            u16::try_from(native.channels().count())?,
            native.rate(),
            "pcm.codec",
        )?;
        Ok(Self { inner, spec })
    }
}

impl FrameCodec for PcmCodec {
    fn decode_frame(
        &mut self,
        frame: Frame<'_>,
        metadata: PacketMetadata,
        out: &mut SampleBuffer,
    ) -> DecodeResult<DecodedPacket> {
        let packet = PacketRef::new(0, Timestamp::ZERO, Duration::new(0), frame.data);
        let decoded = self
            .inner
            .decode_ref(&packet)
            .map_err(DecodeError::backend)?;
        let samples = decoded.samples_interleaved();
        out.ensure_len(samples)?;
        decoded.copy_to_slice_interleaved(&mut out[..samples]);
        out.truncate(samples);
        Ok(DecodedPacket::new(
            u32::try_from(decoded.frames())?,
            metadata,
        ))
    }

    fn prepare_output(&self, out: &mut SampleBuffer) -> DecodeResult<()> {
        let frames =
            self.inner
                .codec_params()
                .max_frames_per_packet
                .ok_or(DecodeError::InvalidData {
                    detail: "PCM track has no packet capacity",
                })?;
        let samples = usize::try_from(frames)?
            .checked_mul(usize::from(self.spec.channels))
            .ok_or(DecodeError::InvalidData {
                detail: "PCM output sample count overflows",
            })?;
        out.ensure_len(samples)?;
        out.clear();
        Ok(())
    }

    fn flush(&mut self) -> DecodeResult<()> {
        self.inner.reset();
        Ok(())
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }
}
