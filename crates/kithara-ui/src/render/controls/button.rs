use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};

use crate::{
    atoms::button::{Button, ButtonConfig, ButtonLabel, VisualState},
    backends::replay_ordered,
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hover, Input, PointerPhase, iced as iced_interact, recognizers::click,
    },
    layout::FrameSides,
    module::ButtonStyle,
    render::{Icon, InputOwner, Mark, ReadValue, Skin, UiEvent, activate},
    text::{TextContext, TextResources},
};

pub(crate) struct ButtonView<'a, 'value, 'data> {
    pub(crate) active_label: Option<&'a str>,
    pub(crate) frame: Option<FrameSides>,
    pub(crate) icon: Option<Icon>,
    pub(crate) label: &'a str,
    pub(crate) owner: InputOwner,
    pub(crate) path: &'a str,
    pub(crate) skin: &'a Skin,
    pub(crate) style: ButtonStyle,
    pub(crate) value: Option<&'value ReadValue<'data>>,
}

/// The icon a button shows at rest, and the one it swaps in while active. Some
/// styles change the icon along with the word, so both are resolved together
/// and a control that flips can repaint instead of being rebuilt.
#[derive(Clone, Copy)]
pub(crate) struct ButtonMarks {
    pub(crate) active: Option<Mark>,
    pub(crate) idle: Option<Mark>,
}

/// Resolves both of a button's icons.
pub(crate) fn button_marks(style: ButtonStyle, icon: Option<Icon>) -> ButtonMarks {
    ButtonMarks {
        active: effective_icon(style, icon, true),
        idle: effective_icon(style, icon, false),
    }
}

pub(crate) fn view<'a>(args: &ButtonView<'a, '_, '_>) -> Element<'a, UiEvent> {
    let active = matches!(args.value, Some(ReadValue::Bool(true)));
    let paint = ButtonPaint::new(
        args.label,
        args.active_label,
        button_marks(args.style, args.icon),
        active,
        args.style,
        args.frame,
        args.skin,
    );
    match args.owner {
        InputOwner::Leaf => ButtonProgram::new(args.path, paint).view(),
        InputOwner::Engine => paint.view(),
    }
}

/// What a button draws once its style has had its say about the document's
/// icon. A style that swaps its icon with the state answers differently for
/// each; the rest answer the same either way.
pub(crate) fn effective_icon(style: ButtonStyle, icon: Option<Icon>, active: bool) -> Option<Mark> {
    if style == ButtonStyle::MicroPrimary {
        return if active { Icon::Pause } else { Icon::Play }.mark();
    }
    icon.and_then(Icon::mark)
}

pub(crate) struct ButtonProgram<'data, 'skin> {
    paint: ButtonPaint<'data, 'skin>,
    path: String,
}

impl<'data, 'skin> ButtonProgram<'data, 'skin> {
    pub(crate) fn new(path: &str, paint: ButtonPaint<'data, 'skin>) -> Self {
        Self {
            paint,
            path: path.to_owned(),
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        let width = self.paint.width();
        Canvas::new(self).width(width).height(Length::Fill).into()
    }
}

impl canvas::Program<UiEvent> for ButtonProgram<'_, '_> {
    type State = ButtonState;

    fn draw(
        &self,
        state: &ButtonState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let hit = iced_interact::hit(bounds, cursor);
        let visual = if state.pressed && hit.over() {
            VisualState::Pressed
        } else if hit.over() {
            VisualState::Hovered
        } else {
            VisualState::Idle
        };
        self.paint
            .draw_with(&state.paint, renderer, theme, bounds, visual)
    }

    fn mouse_interaction(
        &self,
        state: &ButtonState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        Hover::new(CursorShape::Pointer)
            .cursor(state.pressed, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        state: &mut ButtonState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        let action = activate(&self.path, click::on_input(input, &hit));
        let redraw = match input {
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Down && hit.over() => {
                state.hovered = true;
                state.pressed = true;
                false
            }
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Up && state.pressed => {
                state.hovered = hit.over();
                state.pressed = false;
                true
            }
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Move => {
                let hovered = hit.over();
                let changed = state.hovered != hovered;
                state.hovered = hovered;
                changed
            }
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Leave => {
                std::mem::take(&mut state.hovered)
            }
            Input::InputMethod(_)
            | Input::KeyPressed { .. }
            | Input::KeyReleased { .. }
            | Input::ModifiersChanged(_)
            | Input::Pointer(_)
            | Input::Wheel(_) => false,
        };
        action.or_else(|| redraw.then(Action::request_redraw))
    }
}

#[derive(Default)]
pub(crate) struct ButtonState {
    hovered: bool,
    paint: ButtonPaintState,
    pressed: bool,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct ButtonPaint<'data, 'skin> {
    active: bool,
    button: Button,
    label: ButtonLabel<&'data str>,
    text_resources: &'skin TextResources,
    #[field(get, vis = "", copy)]
    width: Length,
}

impl<'data, 'skin> ButtonPaint<'data, 'skin> {
    pub(crate) fn new(
        label: &'data str,
        active_label: Option<&'data str>,
        marks: ButtonMarks,
        active: bool,
        style: ButtonStyle,
        frame: Option<FrameSides>,
        skin: &'skin Skin,
    ) -> Self {
        let button = Button::new(
            ButtonConfig::builder()
                .maybe_frame(frame)
                .maybe_mark(marks.idle)
                .style(style)
                .build(),
            marks.active,
            skin,
        );
        let label = ButtonLabel {
            active: active_label,
            label,
        };
        let width = match style {
            ButtonStyle::Transport => Length::FillPortion(skin.button.transport_fill),
            ButtonStyle::TransportPrimary => Length::FillPortion(skin.button.primary_fill),
            ButtonStyle::MicroPrimary => Length::Fixed(skin.button.micro_size),
            ButtonStyle::VisNav => Length::Fixed(skin.vis.nav_cell_size),
            ButtonStyle::Default => Length::Fixed(button.intrinsic_width(
                &mut TextContext::from(skin.text_resources()),
                &label,
                active,
            )),
        };
        Self {
            active,
            button,
            label,
            text_resources: skin.text_resources(),
            width,
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        let width = self.width();
        Canvas::new(self).width(width).height(Length::Fill).into()
    }

    fn draw_with(
        &self,
        state: &ButtonPaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        visual: VisualState,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let list = self.draw_list(
            state,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
            visual,
        );
        replay_ordered(&list, &mut frame, self.text_resources);
        vec![frame.into_geometry()]
    }

    pub(crate) fn draw_list(
        &self,
        state: &ButtonPaintState,
        bounds: Rect,
        visual: VisualState,
    ) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.button
            .paint(&mut builder, text, &self.label, self.active, bounds, visual);
        builder.finish()
    }
}

impl canvas::Program<UiEvent> for ButtonPaint<'_, '_> {
    type State = ButtonPaintState;

    fn draw(
        &self,
        state: &ButtonPaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let visual = if iced_interact::hit(bounds, cursor).over() {
            VisualState::Hovered
        } else {
            VisualState::Idle
        };
        self.draw_with(state, renderer, theme, bounds, visual)
    }
}

#[derive(Default)]
pub(crate) struct ButtonPaintState {
    text: RefCell<Option<TextContext>>,
}

#[cfg(test)]
mod tests {
    use iced::{Point, event, mouse, window::RedrawRequest};
    use kithara_test_utils::kithara;

    use super::*;
    #[cfg(feature = "masonry-host")]
    use crate::{
        atoms::painter::ButtonData,
        render::masonry::{MasonryControl, Painted},
    };
    use crate::{builtin, render::ControlAction};

    #[kithara::test]
    fn the_leaf_button_uses_the_shared_click_recognizer() {
        let paint = ButtonPaint::new(
            "PLAY",
            Some("PAUSE"),
            ButtonMarks {
                active: None,
                idle: None,
            },
            false,
            ButtonStyle::TransportPrimary,
            None,
            builtin::skin(),
        );
        let program = ButtonProgram::new("transport/play", paint);
        let bounds = Rectangle {
            height: 28.0,
            width: 48.0,
            x: 0.0,
            y: 0.0,
        };
        let cursor = Cursor::Available(Point::new(24.0, 14.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let mut state = ButtonState::default();

        let action = canvas::Program::update(&program, &mut state, &press, bounds, cursor)
            .unwrap_or_else(|| panic!("a button press inside its bounds must publish"));

        assert!(state.pressed);
        assert_eq!(
            action.into_inner(),
            (
                Some(UiEvent::Control {
                    path: "transport/play".to_owned(),
                    action: ControlAction::Activate,
                }),
                RedrawRequest::Wait,
                event::Status::Captured,
            )
        );

        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let action = canvas::Program::update(&program, &mut state, &release, bounds, cursor)
            .unwrap_or_else(|| panic!("releasing a pressed button must repaint it"));
        assert!(!state.pressed);
        assert_eq!(action.into_inner().0, None);
    }

    #[cfg(feature = "masonry-host")]
    #[kithara::test]
    fn iced_and_masonry_record_the_same_button() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 30.0,
            w: 72.0,
            x: 0.0,
            y: 0.0,
        };
        for (style, icon, active) in [
            (ButtonStyle::Default, None, false),
            (ButtonStyle::Default, Some(Icon::Play), false),
            (ButtonStyle::Transport, None, false),
            (ButtonStyle::TransportPrimary, None, true),
            (ButtonStyle::MicroPrimary, Some(Icon::Play), false),
            (ButtonStyle::VisNav, None, false),
        ] {
            let marks = button_marks(style, icon);
            let iced = ButtonPaint::new("PLAY", Some("PAUSE"), marks, active, style, None, skin)
                .draw_list(&ButtonPaintState::default(), bounds, VisualState::Idle);
            let mut masonry = Painted::new(
                Button::new(
                    ButtonConfig::builder()
                        .maybe_mark(marks.idle)
                        .style(style)
                        .build(),
                    marks.active,
                    skin,
                ),
                ButtonData {
                    active,
                    label: ButtonLabel {
                        active: Some("PAUSE".to_owned()),
                        label: "PLAY".to_owned(),
                    },
                },
                skin,
            );

            assert_eq!(
                iced,
                MasonryControl::draw_list(&mut masonry, bounds),
                "the two hosts must record the same button"
            );
        }
    }

    /// The one style that names its own icon keeps naming it, and every other
    /// style now draws the authored art rather than handing it to a toolkit.
    #[kithara::test]
    fn micro_primary_keeps_its_forced_lucide_icon_and_authored_art_is_an_outline() {
        assert!(matches!(
            effective_icon(ButtonStyle::MicroPrimary, Some(Icon::PlayReverse), false),
            Some(Mark::Glyph(glyph)) if Some(glyph) == Icon::Play.lucide_glyph()
        ));
        assert!(matches!(
            effective_icon(ButtonStyle::Default, Some(Icon::PlayReverse), false),
            Some(Mark::Outline(_))
        ));
    }
}
