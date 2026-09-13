use kithara::{
    abr::AbrEvent,
    assets::AssetEvent,
    download::DownloaderEvent,
    play::{DjEvent, EngineEvent, PlayerEvent, SessionEvent},
    queue::QueueEvent,
};
use kithara_audio::{AudioEvent, DecoderEvent};
use kithara_events::EventSet;
use kithara_file::FileEvent;
use kithara_hls::{DrmEvent, HlsEvent};

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
