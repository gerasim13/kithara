use kithara::{abr::AbrEvent, assets::AssetEvent, file::FileEvent, stream::DownloaderEvent};
use kithara_events::{
    AudioEvent, BusEvent, DecoderEvent, EventSet, HlsEvent, ItemEvent, PlayerEvent, QueueEvent,
    TransportEvent,
};

/// Domains inspected by shared integration-test waits and event predicates.
#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub enum TestEvent {
    Abr(AbrEvent),
    Asset(AssetEvent),
    Audio(AudioEvent),
    Bus(BusEvent),
    Decoder(DecoderEvent),
    Downloader(DownloaderEvent),
    File(FileEvent),
    Hls(HlsEvent),
    Item(ItemEvent),
    Player(PlayerEvent),
    Queue(QueueEvent),
    Transport(TransportEvent),
}
