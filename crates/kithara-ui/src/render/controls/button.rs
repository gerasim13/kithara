use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};

use crate::{
    atoms::button::{Button, VisualState},
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, replay},
    interact::{CursorShape, Hover, Input, iced as iced_interact, recognizers::click},
    layout::FrameSides,
    module::ButtonStyle,
    render::{Icon, InputOwner, ReadValue, Skin, UiEvent, activate},
    text::{TextContext, TextResources},
    widgets::{Widget, button::ControlButton},
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

#[derive(Clone, Copy)]
enum EffectiveIcon {
    None,
    Glyph(char),
    Svg(Icon),
}

pub(crate) fn view<'a>(args: &ButtonView<'a, '_, '_>) -> Element<'a, UiEvent> {
    let active = matches!(args.value, Some(ReadValue::Bool(true)));
    let glyph = match effective_icon(args.style, args.icon, active) {
        EffectiveIcon::Glyph(glyph) => Some(glyph),
        EffectiveIcon::None => None,
        EffectiveIcon::Svg(icon) => {
            return ControlButton::builder()
                .path(args.path)
                .label(args.label)
                .icon(icon)
                .maybe_active_label(args.active_label)
                .style(args.style)
                .maybe_frame(args.frame)
                .maybe_value(args.value)
                .skin(args.skin)
                .build()
                .view();
        }
    };
    let paint = ButtonPaint::new(
        args.label,
        args.active_label,
        glyph,
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

fn effective_icon(style: ButtonStyle, icon: Option<Icon>, active: bool) -> EffectiveIcon {
    if style == ButtonStyle::MicroPrimary {
        let icon = if active { Icon::Pause } else { Icon::Play };
        return icon
            .lucide_glyph()
            .map_or(EffectiveIcon::None, EffectiveIcon::Glyph);
    }
    icon.map_or(EffectiveIcon::None, |icon| {
        icon.lucide_glyph()
            .map_or(EffectiveIcon::Svg(icon), EffectiveIcon::Glyph)
    })
}

pub(crate) fn supports_engine_input(style: ButtonStyle, icon: Option<Icon>) -> bool {
    !matches!(effective_icon(style, icon, false), EffectiveIcon::Svg(_))
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
            Input::PointerDown if hit.over() => {
                state.hovered = true;
                state.pressed = true;
                false
            }
            Input::PointerUp if state.pressed => {
                state.hovered = hit.over();
                state.pressed = false;
                true
            }
            Input::PointerMoved { .. } => {
                let hovered = hit.over();
                let changed = state.hovered != hovered;
                state.hovered = hovered;
                changed
            }
            Input::PointerLeft => std::mem::take(&mut state.hovered),
            Input::InputMethod(_)
            | Input::KeyPressed { .. }
            | Input::KeyReleased { .. }
            | Input::ModifiersChanged(_)
            | Input::PointerDown
            | Input::PointerUp
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

pub(crate) struct ButtonPaint<'data, 'skin> {
    button: Button<'data, 'skin>,
    text_resources: &'skin TextResources,
    width: Length,
}

impl<'data, 'skin> ButtonPaint<'data, 'skin> {
    pub(crate) fn new(
        label: &'data str,
        active_label: Option<&'data str>,
        glyph: Option<char>,
        active: bool,
        style: ButtonStyle,
        frame: Option<FrameSides>,
        skin: &'skin Skin,
    ) -> Self {
        let button = Button::builder()
            .active(active)
            .maybe_active_label(active_label)
            .maybe_frame(frame)
            .maybe_glyph(glyph)
            .label(label)
            .style(style)
            .skin(skin)
            .build();
        let width = match style {
            ButtonStyle::Transport => Length::FillPortion(skin.button.transport_fill),
            ButtonStyle::TransportPrimary => Length::FillPortion(skin.button.primary_fill),
            ButtonStyle::MicroPrimary => Length::Fixed(skin.button.micro_size),
            ButtonStyle::VisNav => Length::Fixed(skin.vis.nav_cell_size),
            ButtonStyle::Default => {
                Length::Fixed(button.intrinsic_width(&mut TextContext::from(skin.text_resources())))
            }
        };
        Self {
            button,
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

    fn width(&self) -> Length {
        self.width
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
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.button.paint(
            &mut builder,
            text,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
            visual,
        );
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
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
    use crate::{builtin, render::ControlAction};

    #[kithara::test]
    fn the_leaf_button_uses_the_shared_click_recognizer() {
        let paint = ButtonPaint::new(
            "PLAY",
            Some("PAUSE"),
            None,
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

    #[kithara::test]
    fn micro_primary_keeps_its_forced_lucide_icon_before_capability_routing() {
        assert!(matches!(
            effective_icon(
                ButtonStyle::MicroPrimary,
                Some(Icon::PlayReverse),
                false,
            ),
            EffectiveIcon::Glyph(glyph) if Some(glyph) == Icon::Play.lucide_glyph()
        ));
        assert!(matches!(
            effective_icon(ButtonStyle::Default, Some(Icon::PlayReverse), false),
            EffectiveIcon::Svg(Icon::PlayReverse)
        ));
        assert!(supports_engine_input(
            ButtonStyle::MicroPrimary,
            Some(Icon::PlayReverse)
        ));
        assert!(!supports_engine_input(
            ButtonStyle::Default,
            Some(Icon::PlayReverse)
        ));
    }
}
