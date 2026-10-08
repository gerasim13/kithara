use kithara_events::Event;

use super::PortDescription;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortType {
    BuiltInSpeaker,
    BuiltInReceiver,
    Headphones,
    BluetoothA2dp,
    BluetoothHfp,
    BluetoothLe,
    UsbAudio,
    Hdmi,
    AirPlay,
    LineOut,
    CarAudio,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct RouteDescription {
    pub outputs: Vec<PortDescription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptionKind {
    Began,
    Ended { should_resume: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteChangeReason {
    Unknown,
    NewDeviceAvailable,
    OldDeviceUnavailable,
    CategoryChange,
    Override,
    WakeFromSleep,
    NoSuitableRouteForCategory,
    RouteConfigurationChange,
}

#[derive(Clone, Debug, Event)]
pub enum SessionEvent {
    Interruption {
        kind: InterruptionKind,
    },
    RouteChanged {
        reason: RouteChangeReason,
        previous_route: RouteDescription,
    },
    MediaServicesLost,
    MediaServicesReset,
    SilenceSecondaryAudioHint {
        should_silence: bool,
    },
}
