use kithara_events::{
    AbrEvent, AssetEvent, AudioEvent, DecoderEvent, DjEvent, DownloaderEvent, DrmEvent,
    EngineEvent, EventSet, FileEvent, HlsEvent, PlayerEvent, QueueEvent, SessionEvent,
};

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum QueueBusEvent {
    Player(PlayerEvent),
    Queue(QueueEvent),
    Engine(EngineEvent),
    Session(SessionEvent),
    Dj(DjEvent),
    Asset(AssetEvent),
}

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum ItemBusEvent {
    Decoder(DecoderEvent),
    Audio(AudioEvent),
    Hls(HlsEvent),
    Downloader(DownloaderEvent),
    File(FileEvent),
    Drm(DrmEvent),
    Abr(AbrEvent),
}
