#![cfg(not(target_arch = "wasm32"))]
use super::{DecoderNode, PendingPacket};
use crate::{
    WarpSource,
    test_pools::{TestPools, pools},
    worker::{PcmReceiver, reader_moved_tests::PcmFixture},
};
use kithara_audio::Audio;
use kithara_command::{ChannelConfig, channel};
use kithara_effects::EffectDrain;
use kithara_file::File;
use kithara_signal::AudioChunkInfo;
use kithara_stream::Activity;
use kithara_warp::{Warp, WarpConfig};
use std::num::NonZeroUsize;
pub(super) struct NodeFixture {
    pub(super) node: DecoderNode<Audio<File<TestPools>>, TestPools>,
    pub(super) receiver: PcmReceiver,
    pub(super) activity: Activity,
}
impl NodeFixture {
    pub(super) async fn new(capacity: usize) -> Self {
        let mut fixture = PcmFixture::new(capacity, true).await;
        let mut audio = fixture.audio.take().expect("source owner");
        let activity = audio.activity();
        let writer = audio.take_activity_writer();
        let spec = audio.spec();
        let (_, inbox) = channel(ChannelConfig::builder().build());
        let pools = pools();
        let source = WarpSource::new(
            audio,
            Warp::new((), &WarpConfig::builder().build()).renderer(spec, pools.clone()),
            Vec::new(),
            EffectDrain::new(0, &pools).expect("empty effect drain"),
            spec,
            pools.clone(),
            inbox,
            NonZeroUsize::new(1).expect("preload"),
        );
        Self {
            node: DecoderNode::new(
                source,
                fixture.producer.take().expect("producer owner"),
                writer,
                AudioChunkInfo {
                    spec,
                    ..AudioChunkInfo::default()
                },
                None,
                pools,
            ),
            receiver: fixture.receiver.take().expect("receiver owner"),
            activity,
        }
    }
    pub(super) fn stage(&mut self, packet: crate::worker::PcmPacket) {
        assert!(self.node.pending.is_none(), "one pending owner");
        self.node.pending = Some(PendingPacket {
            packet,
            source_end: None,
        });
    }
}
