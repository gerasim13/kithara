use kithara_audio::SourceEnd;

use crate::PcmPacket;

pub(super) struct PendingPacket {
    pub(super) packet: PcmPacket,
    pub(super) source_end: Option<SourceEnd>,
}
