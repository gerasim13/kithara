use kithara_events::{
    AbrEvent, AssetEvent, AudioEvent, BusEvent, DecoderEvent, DownloaderEvent, EventSet, FileEvent,
    HlsEvent, ItemEvent, PlayerEvent, QueueEvent, TransportEvent,
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
