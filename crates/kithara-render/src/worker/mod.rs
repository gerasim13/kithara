mod config;
mod core;
mod load;
mod node;
mod reader;
#[cfg(test)]
pub(crate) use reader::tests as packet_tests;
pub(crate) mod scheduler;
mod track;

pub use core::{LoadRefusal, PlayWorker};

pub use config::{PlayWorkerConfig, PlayWorkerConfigPatch};
pub use load::{EngineLoad, EngineLoadSnapshot};
pub use node::DecoderNode;
pub use reader::{PcmPacket, PcmReceiver};
pub use track::TrackConfig;

#[cfg(test)]
pub(crate) fn terminal_ring(blocking: bool, spec: kithara_signal::AudioSpec) -> (PcmReceiver, impl FnMut(PcmPacket) -> Result<(), PcmPacket>) {
    use ringbuf::traits::Producer;
    let (receiver, mut producer) = reader::packet_fixture(blocking, spec);
    (receiver, move |packet| {
        let result = producer.forward.try_push(packet);
        producer.signal();
        result
    })
}

#[cfg(test)]
pub(crate) fn terminal_node<T>(source: T, spec: kithara_signal::AudioSpec, blocking: bool) -> (DecoderNode<T, kithara_test_utils::bufpool::TestPools>, PcmReceiver, kithara_command::Sender<crate::LaneProtocol>)
where T: kithara_audio::AudioSource<Chunk = kithara_signal::AudioChunk> {
    let pools = crate::test_pools::pools();
    let (receiver, producer) = reader::packet_fixture(blocking, spec);
    let (sender, inbox) = kithara_command::channel(kithara_command::ChannelConfig::builder().build());
    let config = kithara_warp::WarpConfig::builder().build();
    let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
    let drain = kithara_effects::EffectDrain::new(0, &pools).expect("empty effect drain");
    let warp = crate::WarpSource::new(source, renderer, Vec::new(), drain, spec, pools.clone(), inbox, std::num::NonZeroUsize::MIN);
    let node = DecoderNode::new(warp, producer, None, kithara_signal::AudioChunkInfo { spec, ..Default::default() }, None, pools);
    (node, receiver, sender)
}
