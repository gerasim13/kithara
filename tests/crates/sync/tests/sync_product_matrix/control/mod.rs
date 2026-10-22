mod reference;
mod sampling;

pub(super) use reference::{
    CONTROL_GAIN, DecodedCapture, DecodedObserver, capture, compare_pcm, physical_source_speed,
};
