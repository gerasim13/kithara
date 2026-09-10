use kithara::{abr::AbrEvent, stream::DownloaderEvent};
use kithara_events::{
    AssetEvent, AudioEvent, BusEvent, DecoderEvent, EventSet, FileEvent, HlsEvent, ItemEvent,
    PlayerEvent, QueueEvent, TransportEvent,
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
