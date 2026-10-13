#[cfg(test)]
macro_rules! reset_demuxer_seek {
    ($field:ident = $value:expr) => {
        fn seek(
            &mut self,
            _pos: kithara_platform::time::Duration,
            _priming: crate::codec::CodecPriming,
        ) -> crate::error::DecodeResult<crate::demuxer::DemuxSeekOutcome> {
            self.$field = $value;
            Ok(crate::demuxer::DemuxSeekOutcome::Landed {
                landed_at: kithara_platform::time::Duration::ZERO,
                landed_byte: Some(0),
                preroll: crate::demuxer::PrerollHint::NotNeeded,
            })
        }
    };
}
