mod reference;
mod sampling;

pub(super) use reference::{
    CONTROL_GAIN, DecodedCapture, DecodedRecorder, capture, compare_pcm, physical_source_speed,
};
