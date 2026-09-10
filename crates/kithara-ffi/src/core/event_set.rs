use kithara::{
    abr::AbrEvent,
    assets::AssetEvent,
    audio::{AudioEvent, DecoderEvent},
    stream::DownloaderEvent,
};
use kithara_events::{
    DjEvent, DrmEvent, EngineEvent, EventSet, HlsEvent, PlayerEvent, QueueEvent, SessionEvent,
};
use kithara_file::FileEvent;

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
