use std::rc::Rc;

use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use super::custom::{HostAction, Repaint};
use crate::{
    atoms::{
        button::Button,
        chip::Chip,
        design::{
            cell::Cell, crossfader::Crossfader, fader::Fader, meter::Meter, status_dot::StatusDot,
            swatch::Swatch,
        },
        knob::Knob,
        meter::StereoMeter,
        nav_item::NavItem,
        painter::{ControlPainter, Labelled},
        tab::TabLarge,
        toggle::Binary,
        vu::VerticalVu,
    },
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome, PointerOwnership, PointerPhase,
        recognizers::{Scalar, ScalarState, Track, WheelStep, click},
    },
    render::{
        ControlAction, ReadValue, Skin, StereoLevels, UiEvent, control_event, controls::Press,
    },
    text::TextContext,
};

/// One built-in control mounted as a Masonry leaf.
///
/// Built-ins take this route rather than the public custom-component contract
/// because they need direct cursor and capture ownership, which that contract
/// does not expose.
pub(crate) trait MasonryControl {
    fn draw_list(&mut self, bounds: Rect) -> DrawList;

    /// How big the control is on the axes it settles for itself. A zero on an
    /// axis leaves it to the row, which is what a leaf that cannot measure has
    /// always answered on both.
    fn measure(&mut self) -> crate::solve::Size {
        crate::solve::Size::ZERO
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction>;

    fn accepts_input(&self) -> bool;

    /// Takes what the control's endpoint now reads, and reports whether the
    /// control draws differently for it. This is the one way a mounted control
    /// learns a new value; no control is special-cased anywhere above it.
    fn set_read(&mut self, _value: &ReadValue<'_>) -> bool {
        false
    }

    fn cursor(&self, _hit: &Hit) -> CursorShape {
        CursorShape::None
    }

    /// Takes the hover edge the host observed and reports whether the control
    /// now draws differently.
    fn hover(&mut self, _hovered: bool) -> bool {
        false
    }

    fn repaint(&self) -> Repaint {
        Repaint::None
    }
}

pub(crate) struct MasonryKnob {
    input: Option<KnobInput>,
    label: Option<String>,
    painter: Knob,
    repaint: bool,
    text: TextContext,
    value: f32,
}

struct KnobInput {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
    recognizer: Scalar,
    state: ScalarState,
}

impl MasonryKnob {
    pub(crate) fn new(label: Option<String>, value: f32, skin: &Skin) -> Self {
        Self {
            input: None,
            label,
            painter: Knob::new(skin),
            repaint: false,
            text: TextContext::from(skin.text_resources()),
            value,
        }
    }

    pub(crate) fn interactive(
        mut self,
        path: String,
        track: Track,
        wheel: WheelStep,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        self.input = Some(KnobInput {
            map_event,
            path,
            recognizer: Scalar::builder()
                .track(track)
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(0.5)
                .wheel(wheel)
                .build(),
            state: ScalarState::default(),
        });
        self
    }
}

impl MasonryControl for MasonryKnob {
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        self.repaint = false;
        let mut list = DrawListBuilder::default();
        self.painter.paint(
            &mut list,
            &mut self.text,
            self.value,
            self.label.as_deref(),
            bounds,
        );
        list.finish()
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        let Some(state) = &mut self.input else {
            return Outcome::IGNORED;
        };
        let had_pointer = state.state.captures_pointer();
        let outcome = state
            .recognizer
            .on_input(&mut state.state, input, hit, Instant::now());
        if matches!(
            input,
            Input::Pointer(pointer)
                if matches!(pointer.phase, PointerPhase::Cancel | PointerPhase::DoubleClick)
        ) {
            state.state.cancel_pointer();
        }
        let ownership = match (had_pointer, state.state.captures_pointer()) {
            (false, true) => PointerOwnership::Claim,
            (true, false) => PointerOwnership::Release,
            _ => PointerOwnership::Unchanged,
        };
        // The knob draws the value it just authored: the application is told the
        // same number, but its answer only reaches this leaf on the rebuild that
        // follows the gesture.
        if let Some(value) = outcome.value() {
            self.repaint |= (self.value - value).abs() > f32::EPSILON;
            self.value = value;
        }
        let Some(state) = &self.input else {
            return Outcome::IGNORED;
        };
        outcome.with_ownership(ownership).map(|value| {
            (state.map_event)(control_event(
                &state.path,
                ControlAction::SetScalar(f64::from(value)),
            ))
        })
    }

    fn accepts_input(&self) -> bool {
        self.input.is_some()
    }

    fn set_read(&mut self, value: &ReadValue<'_>) -> bool {
        let ReadValue::Scalar(value) = value else {
            return false;
        };
        let value = AsPrimitive::<f32>::as_(value.clamp(0.0, 1.0));
        self.repaint |= (self.value - value).abs() > f32::EPSILON;
        self.value = value;
        self.repaint
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.input.as_ref().map_or(CursorShape::None, |input| {
            input.recognizer.cursor(&input.state, hit)
        })
    }

    fn repaint(&self) -> Repaint {
        if self.repaint {
            Repaint::NextFrame
        } else {
            Repaint::None
        }
    }
}

#[cfg(test)]
mod flags {
    use kithara_test_utils::kithara;

    use super::{MasonryControl, Painted};
    use crate::{
        atoms::{
            button::{Button, ButtonConfig, ButtonLabel},
            chip::Chip,
            nav_item::NavItem,
            painter::{ButtonData, Labelled, NavData},
            tab::TabLarge,
        },
        builtin,
        draw::Rect,
        module::{ButtonStyle, ChipStyle},
        render::{Mark, ReadValue, Skin},
    };

    /// Every control whose picture is decided by a flag. A control that reads a
    /// flag and cannot be told the flag changed can only catch up by being
    /// rebuilt, and a rebuild throws away the gesture the hand is in the middle
    /// of, the pointer capture feeding it, and the run of clicks that makes a
    /// double click. So each one here must answer `set_read` and draw
    /// differently for it.
    #[kithara::test]
    fn a_flag_bound_control_redraws_when_its_endpoint_flips() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 120.0,
            x: 0.0,
            y: 0.0,
        };
        let mark = Mark::Glyph(char::from(lucide_icons::Icon::Disc));
        let labelled = || Labelled {
            active: false,
            label: "MIXER".to_owned(),
        };
        let mounted: [(&str, Box<dyn MasonryControl>); 4] = [
            (
                "Chip",
                Box::new(Painted::new(
                    Chip::new(ChipStyle::Deck, skin),
                    labelled(),
                    skin,
                )),
            ),
            (
                "NavItem",
                Box::new(Painted::new(
                    NavItem::new(skin),
                    NavData {
                        active: false,
                        label: "MIXER".to_owned(),
                        mark,
                    },
                    skin,
                )),
            ),
            (
                "TabLarge",
                Box::new(Painted::new(TabLarge::new(skin), labelled(), skin)),
            ),
            ("Button", Box::new(button(skin))),
        ];

        for (name, mut control) in mounted {
            let idle = control.draw_list(bounds);
            assert!(
                control.set_read(&ReadValue::Bool(true)),
                "{name} must report that flipping its endpoint changed its picture"
            );
            assert_ne!(
                idle,
                control.draw_list(bounds),
                "{name} drew the same picture after its endpoint flipped, so only a rebuild \
                 could ever show the new state"
            );
        }
    }

    fn button(skin: &Skin) -> Painted<Button> {
        Painted::new(
            Button::new(
                ButtonConfig::builder()
                    .style(ButtonStyle::TransportPrimary)
                    .build(),
                None,
                skin,
            ),
            ButtonData {
                active: false,
                label: ButtonLabel {
                    active: Some("PAUSE".to_owned()),
                    label: "PLAY".to_owned(),
                },
            },
            skin,
        )
    }
}

/// The half of the painter contract only a host that keeps its widgets needs.
///
/// A host that rebuilds its tree on every message learns a new value by being
/// rebuilt. A retained host cannot: rebuilding would throw away the gesture the
/// hand is in the middle of, the pointer capture feeding it, and the run of
/// clicks a double click is made of. So it tells the mounted control instead,
/// through here.
pub(crate) trait Retained: ControlPainter {
    /// Puts what the endpoint now reads into the data this painter draws, and
    /// says whether that changed the picture.
    fn set_read(_data: &mut Self::Data, _value: &ReadValue<'_>) -> bool {
        false
    }
}

/// Takes a flag off an endpoint.
fn set_bool(data: &mut bool, value: &ReadValue<'_>) -> bool {
    let ReadValue::Bool(active) = value else {
        return false;
    };
    std::mem::replace(data, *active) != *active
}

/// Takes a fraction off an endpoint, clamped to the unit range every painter
/// draws in.
fn set_scalar(data: &mut f32, value: &ReadValue<'_>) -> bool {
    let ReadValue::Scalar(value) = value else {
        return false;
    };
    let value = AsPrimitive::<f32>::as_(value.clamp(0.0, 1.0));
    std::mem::replace(data, value) != value
}

/// A meter shows the levels it reads; the scalar it publishes while dragged is
/// its volume, which is the one part of those levels a hand can set.
fn set_levels(data: &mut StereoLevels, value: &ReadValue<'_>) -> bool {
    let levels = match value {
        ReadValue::Stereo(levels) => *levels,
        ReadValue::Scalar(volume) => StereoLevels {
            volume: AsPrimitive::<f32>::as_(volume.clamp(0.0, 1.0)),
            ..*data
        },
        _ => return false,
    };
    std::mem::replace(data, levels) != levels
}

/// A word and a state: the flag decides the picture, and a text endpoint may
/// supply the word.
fn set_labelled(data: &mut Labelled, value: &ReadValue<'_>) -> bool {
    match value {
        ReadValue::Bool(active) => set_bool(&mut data.active, value) || *active != data.active,
        ReadValue::Text(label) => {
            *label != data.label && {
                data.label = (*label).to_owned();
                true
            }
        }
        _ => false,
    }
}

impl Retained for Chip {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_labelled(data, value)
    }
}

impl Retained for NavItem {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_bool(&mut data.active, value)
    }
}

impl Retained for TabLarge {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_labelled(data, value)
    }
}

impl Retained for Fader {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_scalar(&mut data.value, value)
    }
}

impl Retained for Crossfader {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_scalar(data, value)
    }
}

impl Retained for Meter {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_scalar(data, value)
    }
}

/// A status dot and a cell show what the document said; no endpoint moves
/// either of them.
impl Retained for StatusDot {}

impl Retained for Cell {}

impl Retained for Swatch {}

impl Retained for Binary {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_bool(data, value)
    }
}

impl Retained for VerticalVu {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_levels(data, value)
    }
}

impl Retained for StereoMeter {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_levels(data, value)
    }
}

impl Retained for Button {
    fn set_read(data: &mut Self::Data, value: &ReadValue<'_>) -> bool {
        set_bool(&mut data.active, value)
    }
}

/// One built-in control mounted as a Masonry leaf: a painter, the data it
/// draws, and the press that may activate it.
pub(crate) struct Painted<Painter>
where
    Painter: Retained,
{
    activation: Option<Activation>,
    data: Painter::Data,
    painter: Painter,
    press: Press,
    repaint: bool,
    text: TextContext,
}

/// Path and event mapping for a control that owns its own press.
struct Activation {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
}

impl<Painter> Painted<Painter>
where
    Painter: Retained,
{
    pub(crate) fn new(painter: Painter, data: Painter::Data, skin: &Skin) -> Self {
        Self {
            activation: None,
            data,
            painter,
            press: Press::default(),
            repaint: false,
            text: TextContext::from(skin.text_resources()),
        }
    }

    pub(crate) fn interactive(
        mut self,
        path: String,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        self.activation = Some(Activation { map_event, path });
        self
    }
}

impl<Painter> MasonryControl for Painted<Painter>
where
    Painter: Retained,
{
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        self.repaint = false;
        let mut list = DrawListBuilder::default();
        self.painter.draw(
            &mut list,
            &mut self.text,
            &self.data,
            bounds,
            self.press.visual(),
        );
        list.finish()
    }

    fn measure(&mut self) -> crate::solve::Size {
        self.painter.measure(&mut self.text, &self.data)
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        if Painter::READS_POINTER {
            self.repaint |= self.press.press(input, hit);
        }
        self.activation
            .as_ref()
            .map_or(Outcome::IGNORED, |activation| {
                click::on_input(input, hit).map(|()| {
                    (activation.map_event)(control_event(&activation.path, ControlAction::Activate))
                })
            })
    }

    fn accepts_input(&self) -> bool {
        self.activation.is_some()
    }

    fn set_read(&mut self, value: &ReadValue<'_>) -> bool {
        self.repaint |= Painter::set_read(&mut self.data, value);
        self.repaint
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.activation.as_ref().map_or(CursorShape::None, |_| {
            Hover::new(CursorShape::Pointer).cursor(self.press.is_pressed(), hit)
        })
    }

    fn hover(&mut self, hovered: bool) -> bool {
        if !Painter::READS_POINTER {
            return false;
        }
        self.repaint |= self.press.hover(hovered);
        self.repaint
    }

    fn repaint(&self) -> Repaint {
        if self.repaint {
            Repaint::NextFrame
        } else {
            Repaint::None
        }
    }
}
