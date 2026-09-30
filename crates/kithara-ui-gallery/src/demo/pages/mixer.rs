use kithara_ui::{
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{ReadValue, StereoLevels, WriteValue},
};
use num_traits::cast::AsPrimitive;

use crate::demo::DemoRegistry;

pub(crate) struct MixerState {
    fx: [[bool; 2]; 2],
    standard: [[f64; 4]; 2],
    xone: [[f64; 4]; 2],
    cue: [bool; 2],
    faders: [f64; 2],
    crossfader: f64,
}

impl Default for MixerState {
    fn default() -> Self {
        Self {
            crossfader: 0.5,
            cue: [true, false],
            faders: [0.76, 0.68],
            fx: [[true, false], [false, true]],
            standard: [[0.68, 0.52, 0.44, 0.61], [0.57, 0.48, 0.63, 0.39]],
            xone: [[0.66, 0.54, 0.46, 0.58], [0.55, 0.43, 0.62, 0.49]],
        }
    }
}

impl MixerState {
    const A: usize = 0;
    const B: usize = 1;
    const FILTER: usize = 3;
    const FX1: usize = 0;
    const FX2: usize = 1;
    const HI: usize = 0;
    const HI_MID: usize = 1;
    const LO: usize = 2;
    const LO_MID: usize = 2;
    const MID: usize = 1;
    const XONE_LO: usize = 3;

    pub(crate) fn write(&mut self, id: &str, value: &WriteValue) {
        match value {
            WriteValue::Trigger => {
                if let Some(flag) = self.flag(id) {
                    *flag = !*flag;
                }
            }
            WriteValue::Scalar(value) => {
                if let Some(scalar) = self.scalar(id) {
                    *scalar = value.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn flag(&mut self, id: &str) -> Option<&mut bool> {
        Some(match id {
            "mixer.channel.a.toggle_cue" => &mut self.cue[Self::A],
            "mixer.channel.b.toggle_cue" => &mut self.cue[Self::B],
            "mixer.channel.a.toggle_fx1" => &mut self.fx[Self::A][Self::FX1],
            "mixer.channel.a.toggle_fx2" => &mut self.fx[Self::A][Self::FX2],
            "mixer.channel.b.toggle_fx1" => &mut self.fx[Self::B][Self::FX1],
            "mixer.channel.b.toggle_fx2" => &mut self.fx[Self::B][Self::FX2],
            _ => return None,
        })
    }

    pub(crate) fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let value = match endpoint {
            "mixer.standard.a.hi" => ReadValue::Scalar(self.standard[Self::A][Self::HI]),
            "mixer.standard.a.mid" => ReadValue::Scalar(self.standard[Self::A][Self::MID]),
            "mixer.standard.a.lo" => ReadValue::Scalar(self.standard[Self::A][Self::LO]),
            "mixer.standard.a.filter" => ReadValue::Scalar(self.standard[Self::A][Self::FILTER]),
            "mixer.standard.b.hi" => ReadValue::Scalar(self.standard[Self::B][Self::HI]),
            "mixer.standard.b.mid" => ReadValue::Scalar(self.standard[Self::B][Self::MID]),
            "mixer.standard.b.lo" => ReadValue::Scalar(self.standard[Self::B][Self::LO]),
            "mixer.standard.b.filter" => ReadValue::Scalar(self.standard[Self::B][Self::FILTER]),
            "mixer.xone.a.hi" => ReadValue::Scalar(self.xone[Self::A][Self::HI]),
            "mixer.xone.a.hi_mid" => ReadValue::Scalar(self.xone[Self::A][Self::HI_MID]),
            "mixer.xone.a.lo_mid" => ReadValue::Scalar(self.xone[Self::A][Self::LO_MID]),
            "mixer.xone.a.lo" => ReadValue::Scalar(self.xone[Self::A][Self::XONE_LO]),
            "mixer.xone.b.hi" => ReadValue::Scalar(self.xone[Self::B][Self::HI]),
            "mixer.xone.b.hi_mid" => ReadValue::Scalar(self.xone[Self::B][Self::HI_MID]),
            "mixer.xone.b.lo_mid" => ReadValue::Scalar(self.xone[Self::B][Self::LO_MID]),
            "mixer.xone.b.lo" => ReadValue::Scalar(self.xone[Self::B][Self::XONE_LO]),
            "mixer.channel.a.fx1" => ReadValue::Bool(self.fx[Self::A][Self::FX1]),
            "mixer.channel.a.fx2" => ReadValue::Bool(self.fx[Self::A][Self::FX2]),
            "mixer.channel.b.fx1" => ReadValue::Bool(self.fx[Self::B][Self::FX1]),
            "mixer.channel.b.fx2" => ReadValue::Bool(self.fx[Self::B][Self::FX2]),
            "mixer.channel.a.cue" => ReadValue::Bool(self.cue[Self::A]),
            "mixer.channel.b.cue" => ReadValue::Bool(self.cue[Self::B]),
            "mixer.channel.a.fader" => ReadValue::Scalar(self.faders[Self::A]),
            "mixer.channel.b.fader" => ReadValue::Scalar(self.faders[Self::B]),
            "mixer.channel.a.levels" => ReadValue::Stereo(self.levels(Self::A)),
            "mixer.channel.b.levels" => ReadValue::Stereo(self.levels(Self::B)),
            "mixer.xfade" => ReadValue::Scalar(self.crossfader),
            _ => return None,
        };
        Some(value)
    }

    fn levels(&self, channel: usize) -> StereoLevels {
        let (l, r) = if channel == Self::A {
            (0.82, 0.71)
        } else {
            (0.63, 0.74)
        };
        StereoLevels {
            l,
            r,
            volume: self.faders[channel].as_(),
        }
    }

    fn scalar(&mut self, id: &str) -> Option<&mut f64> {
        Some(match id {
            "mixer.standard.a.hi" => &mut self.standard[Self::A][Self::HI],
            "mixer.standard.a.mid" => &mut self.standard[Self::A][Self::MID],
            "mixer.standard.a.lo" => &mut self.standard[Self::A][Self::LO],
            "mixer.standard.a.filter" => &mut self.standard[Self::A][Self::FILTER],
            "mixer.standard.b.hi" => &mut self.standard[Self::B][Self::HI],
            "mixer.standard.b.mid" => &mut self.standard[Self::B][Self::MID],
            "mixer.standard.b.lo" => &mut self.standard[Self::B][Self::LO],
            "mixer.standard.b.filter" => &mut self.standard[Self::B][Self::FILTER],
            "mixer.xone.a.hi" => &mut self.xone[Self::A][Self::HI],
            "mixer.xone.a.hi_mid" => &mut self.xone[Self::A][Self::HI_MID],
            "mixer.xone.a.lo_mid" => &mut self.xone[Self::A][Self::LO_MID],
            "mixer.xone.a.lo" => &mut self.xone[Self::A][Self::XONE_LO],
            "mixer.xone.b.hi" => &mut self.xone[Self::B][Self::HI],
            "mixer.xone.b.hi_mid" => &mut self.xone[Self::B][Self::HI_MID],
            "mixer.xone.b.lo_mid" => &mut self.xone[Self::B][Self::LO_MID],
            "mixer.xone.b.lo" => &mut self.xone[Self::B][Self::XONE_LO],
            "mixer.channel.a.fader" => &mut self.faders[Self::A],
            "mixer.channel.b.fader" => &mut self.faders[Self::B],
            "mixer.xfade" => &mut self.crossfader,
            _ => return None,
        })
    }
}

pub(crate) fn insert_endpoints(registry: &mut DemoRegistry) {
    for id in [
        "mixer.standard.a.hi",
        "mixer.standard.a.mid",
        "mixer.standard.a.lo",
        "mixer.standard.a.filter",
        "mixer.standard.b.hi",
        "mixer.standard.b.mid",
        "mixer.standard.b.lo",
        "mixer.standard.b.filter",
        "mixer.xone.a.hi",
        "mixer.xone.a.hi_mid",
        "mixer.xone.a.lo_mid",
        "mixer.xone.a.lo",
        "mixer.xone.b.hi",
        "mixer.xone.b.hi_mid",
        "mixer.xone.b.lo_mid",
        "mixer.xone.b.lo",
        "mixer.channel.a.fader",
        "mixer.channel.b.fader",
        "mixer.xfade",
    ] {
        registry.insert(
            EndpointCategory::Parameter,
            id,
            EndpointDesc::new(ValueKind::Scalar),
        );
    }
    for id in [
        "mixer.channel.a.fx1",
        "mixer.channel.a.fx2",
        "mixer.channel.b.fx1",
        "mixer.channel.b.fx2",
        "mixer.channel.a.cue",
        "mixer.channel.b.cue",
    ] {
        registry.insert(
            EndpointCategory::Model,
            id,
            EndpointDesc::new(ValueKind::Bool),
        );
    }
    for id in ["mixer.channel.a.levels", "mixer.channel.b.levels"] {
        registry.insert(
            EndpointCategory::Telemetry,
            id,
            EndpointDesc::new(ValueKind::Stereo),
        );
    }
    for id in [
        "mixer.channel.a.toggle_fx1",
        "mixer.channel.a.toggle_fx2",
        "mixer.channel.b.toggle_fx1",
        "mixer.channel.b.toggle_fx2",
        "mixer.channel.a.toggle_cue",
        "mixer.channel.b.toggle_cue",
    ] {
        registry.insert(
            EndpointCategory::Command,
            id,
            EndpointDesc::new(ValueKind::Trigger),
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn crossfader_and_channel_faders_persist_scalar_writes() {
        let mut mixer = MixerState::default();

        assert_eq!(mixer.get("mixer.xfade"), Some(ReadValue::Scalar(0.5)));
        mixer.write("mixer.xfade", &WriteValue::Scalar(0.27));
        assert_eq!(mixer.get("mixer.xfade"), Some(ReadValue::Scalar(0.27)));

        mixer.write("mixer.channel.a.fader", &WriteValue::Scalar(0.41));
        assert_eq!(
            mixer.get("mixer.channel.a.fader"),
            Some(ReadValue::Scalar(0.41))
        );
        assert!(matches!(
            mixer.get("mixer.channel.a.levels"),
            Some(ReadValue::Stereo(StereoLevels { volume, .. })) if volume == 0.41
        ));
    }

    #[kithara::test]
    fn cue_and_fx_activation_persist_bool_reads() {
        let mut mixer = MixerState::default();

        assert_eq!(
            mixer.get("mixer.channel.a.cue"),
            Some(ReadValue::Bool(true))
        );
        mixer.write("mixer.channel.a.toggle_cue", &WriteValue::Trigger);
        assert_eq!(
            mixer.get("mixer.channel.a.cue"),
            Some(ReadValue::Bool(false))
        );
        mixer.write("mixer.channel.b.toggle_fx1", &WriteValue::Trigger);
        assert_eq!(
            mixer.get("mixer.channel.b.fx1"),
            Some(ReadValue::Bool(true))
        );
    }
}
