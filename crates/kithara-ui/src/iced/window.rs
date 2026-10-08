use iced::Element;

use crate::{
    draw::{DrawList, Pt, Rect},
    hosts::{
        layer::{HostLayer, LayerHit, WindowLayerProgram},
        solve::{Length, Size},
        window::{
            controls::{ControlsProgram as SharedControlsProgram, ControlsState},
            title::TitleProgram,
        },
    },
    iced::layer::IcedWindowLayerProgram,
    interact::{CursorShape, Input, Outcome},
    module::WindowControlsStyle,
    render::{Skin, WindowCommand},
    shaping::TextResources,
    skin::WindowControlSkin,
};

#[derive(bon::Builder)]
pub(crate) struct WindowControls<'skin> {
    skin: &'skin Skin,
    style: WindowControlsStyle,
}

impl<'a> crate::iced::tree::Widget<'a> for WindowControls<'_> {
    fn view(self) -> Element<'a, crate::render::Published> {
        crate::iced::layer::window_layer(ControlsProgram::new(self.style, self.skin))
    }
}

#[derive(bon::Builder)]
pub(crate) struct TitleBar<'label, 'skin> {
    skin: &'skin Skin,
    label: &'label str,
}

impl<'a> crate::iced::tree::Widget<'a> for TitleBar<'_, '_> {
    fn view(self) -> Element<'a, crate::render::Published> {
        crate::iced::layer::window_layer(TitleProgram::new(self.label, self.skin))
    }
}

impl<'a> crate::iced::tree::Widget<'a> for WindowSurface {
    fn view(self) -> Element<'a, crate::render::Published> {
        crate::iced::layer::window_layer(self.program())
    }
}

impl WindowSurface {
    const fn program(&self) -> SurfaceProgram {
        SurfaceProgram {
            command: self.command,
            cursor: self.cursor,
            height: self.height,
            width: self.width,
        }
    }
}

struct SurfaceProgram {
    cursor: CursorShape,
    height: Length,
    width: Length,
    command: WindowCommand,
}

impl WindowLayerProgram for SurfaceProgram {
    type State = ();

    fn layer(&self, _state: &(), bounds: Rect, _pointer: Option<Pt>) -> HostLayer<WindowCommand> {
        HostLayer::new(
            bounds,
            DrawList::default(),
            vec![LayerHit::new(bounds, self.cursor, self.command)],
        )
    }
}

#[cfg(test)]
mod tests {
    use iced::{event, window::RedrawRequest};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        draw::{Pt, Rect},
        iced::tree::window,
        interact::{CursorShape, Input, Outcome, PointerPhase, mouse as mouse_input},
        render::{Published, WindowCommand, WindowEdge},
    };

    fn pointer_down() -> Input<'static> {
        Input::Pointer(mouse_input(PointerPhase::Down, None))
    }

    /// The retained host's control census calls the window-drag region a
    /// control with no picture rather than one still waiting for a painter.
    /// That is only honest while this host draws nothing for it either, and
    /// while the region still earns its place by carrying the window.
    #[kithara::test]
    fn a_drag_surface_carries_the_window_and_draws_nothing() {
        let bounds = Rect {
            h: 40.0,
            w: 200.0,
            x: 0.0,
            y: 0.0,
        };
        let pointer = Some(Pt { x: 10.0, y: 10.0 });
        let layer = WindowSurface::drag().program().layer(&(), bounds, pointer);

        assert!(layer.draw().commands().is_empty());
        assert_eq!(layer.action_at(pointer), Some(&WindowCommand::Drag));
    }

    #[kithara::test]
    fn every_resize_edge_has_its_exact_region_command_and_cursor() {
        let layer = crate::hosts::window::surface::frame(
            Rect {
                h: 60.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
            4.0,
        );
        let expected = [
            (
                Rect {
                    h: 4.0,
                    w: 4.0,
                    x: 0.0,
                    y: 0.0,
                },
                WindowEdge::NorthWest,
                CursorShape::ResizeDiagonalDown,
            ),
            (
                Rect {
                    h: 4.0,
                    w: 92.0,
                    x: 4.0,
                    y: 0.0,
                },
                WindowEdge::North,
                CursorShape::ResizeV,
            ),
            (
                Rect {
                    h: 4.0,
                    w: 4.0,
                    x: 96.0,
                    y: 0.0,
                },
                WindowEdge::NorthEast,
                CursorShape::ResizeDiagonalUp,
            ),
            (
                Rect {
                    h: 52.0,
                    w: 4.0,
                    x: 0.0,
                    y: 4.0,
                },
                WindowEdge::West,
                CursorShape::ResizeH,
            ),
            (
                Rect {
                    h: 52.0,
                    w: 4.0,
                    x: 96.0,
                    y: 4.0,
                },
                WindowEdge::East,
                CursorShape::ResizeH,
            ),
            (
                Rect {
                    h: 4.0,
                    w: 4.0,
                    x: 0.0,
                    y: 56.0,
                },
                WindowEdge::SouthWest,
                CursorShape::ResizeDiagonalUp,
            ),
            (
                Rect {
                    h: 4.0,
                    w: 92.0,
                    x: 4.0,
                    y: 56.0,
                },
                WindowEdge::South,
                CursorShape::ResizeV,
            ),
            (
                Rect {
                    h: 4.0,
                    w: 4.0,
                    x: 96.0,
                    y: 56.0,
                },
                WindowEdge::SouthEast,
                CursorShape::ResizeDiagonalDown,
            ),
        ];

        assert_eq!(layer.hits().len(), expected.len());
        for (hit, (area, edge, cursor)) in layer.hits().iter().zip(expected) {
            assert_eq!(hit.area(), area, "wrong area for {edge:?}");
            assert_eq!(hit.action(), &WindowCommand::Resize(edge));
            assert_eq!(hit.cursor(), cursor, "wrong cursor for {edge:?}");
            let pointer = Pt {
                x: area.x + area.w / 2.0,
                y: area.y + area.h / 2.0,
            };
            let outcome = layer.handle(pointer_down(), Some(pointer));
            assert_eq!(
                outcome,
                Outcome::set(WindowCommand::Resize(edge)),
                "wrong command emitted for {edge:?}"
            );
            let action = window(WindowCommand::Resize(edge), outcome.map(|_| ()))
                .unwrap_or_else(|| panic!("{edge:?} must bind to a window event"));
            assert_eq!(
                action.into_inner(),
                (
                    Some(Published::window(WindowCommand::Resize(edge))),
                    RedrawRequest::Wait,
                    event::Status::Captured,
                ),
                "wrong event bound for {edge:?}"
            );
            assert_eq!(
                layer.cursor_at(Some(pointer)),
                cursor,
                "wrong cursor reported for {edge:?}"
            );
        }
    }

    #[kithara::test]
    fn the_drag_surface_emits_the_window_drag_command() {
        let surface = WindowSurface::drag();
        let program = SurfaceProgram {
            command: surface.command,
            cursor: surface.cursor,
            height: surface.height,
            width: surface.width,
        };
        let bounds = Rect {
            h: 32.0,
            w: 180.0,
            x: 17.0,
            y: 9.0,
        };
        let pointer = Some(Pt { x: 90.0, y: 16.0 });
        let layer = program.layer(&(), bounds, None);
        let (outcome, redraw) = program.update(&mut (), pointer_down(), &layer, pointer);

        assert_eq!(outcome, Outcome::set(WindowCommand::Drag));
        assert!(!redraw);
        assert_eq!(layer.bounds(), bounds);
        assert_eq!(layer.hits()[0].area(), bounds);
        assert_eq!(layer.cursor_at(pointer), CursorShape::None);
    }
}
impl SharedControlsProgram {
    pub(crate) fn height(&self) -> Length {
        match self.controls {
            WindowControlSkin::Close {
                cell_size,
                divider: Some(_),
                ..
            } => Length::Fixed(cell_size),
            WindowControlSkin::Buttons { .. } | WindowControlSkin::Close { .. } => Length::Fill,
        }
    }

    pub(crate) fn width(&self) -> f32 {
        match self.controls {
            WindowControlSkin::Buttons {
                minus_icon_size,
                maximize_icon_size,
                close_icon_size,
                gap,
                padding,
            } => minus_icon_size + maximize_icon_size + close_icon_size + gap * 2.0 + padding * 2.0,
            WindowControlSkin::Close { cell_size, .. } => cell_size,
        }
    }

    pub(crate) fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width()), self.height())
    }
}

struct ControlsProgram {
    program: SharedControlsProgram,
    resources: TextResources,
}

impl ControlsProgram {
    fn new(style: WindowControlsStyle, skin: &Skin) -> Self {
        Self {
            program: SharedControlsProgram::new(style, skin),
            resources: skin.text_resources.as_ref().clone(),
        }
    }
}

impl WindowLayerProgram for ControlsProgram {
    type State = ControlsState;

    delegate::delegate! {
        to self.program {
            fn hit_layer(&self, state: &ControlsState, bounds: Rect) -> HostLayer<WindowCommand>;
            fn layer(&self, state: &ControlsState, bounds: Rect, pointer: Option<Pt>) -> HostLayer<WindowCommand>;
            fn update(&self, state: &mut ControlsState, input: Input<'_>, layer: &HostLayer<WindowCommand>, pointer: Option<Pt>) -> (Outcome<WindowCommand>, bool);
        }
    }
}

impl IcedWindowLayerProgram for ControlsProgram {
    fn resources(&self) -> Option<&TextResources> {
        Some(&self.resources)
    }

    fn size(&self) -> Size<Length> {
        self.program.size()
    }
}

impl IcedWindowLayerProgram for TitleProgram {
    fn resources(&self) -> Option<&TextResources> {
        Some(&self.resources)
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
}

impl IcedWindowLayerProgram for SurfaceProgram {
    fn resources(&self) -> Option<&TextResources> {
        None
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }
}

pub(crate) struct WindowSurface {
    pub(crate) cursor: CursorShape,
    pub(crate) height: Length,
    pub(crate) width: Length,
    pub(crate) command: WindowCommand,
}

impl WindowSurface {
    pub(crate) const fn drag() -> Self {
        Self {
            command: WindowCommand::Drag,
            width: Length::Fill,
            height: Length::Fill,
            cursor: CursorShape::None,
        }
    }
}

#[cfg(test)]
mod controls_tests {
    use kithara_test_utils::kithara;

    use super::{ControlsState, SharedControlsProgram as ControlsProgram, WindowLayerProgram};
    use crate::{
        builtin,
        draw::{DrawCmd, Geom, Paint, Pt, Rect},
        hosts::solve::{Length, Size},
        interact::{CursorShape, Input, Outcome, PointerPhase, mouse as mouse_input},
        module::WindowControlsStyle,
        render::WindowCommand,
        skin::WindowControlSkin,
    };

    fn pointer_input(phase: PointerPhase, at: Option<Pt>) -> Input<'static> {
        Input::Pointer(mouse_input(phase, at))
    }

    #[kithara::test]
    fn styles_select_their_skin_metrics() {
        let window = builtin::skin_doc().window;

        assert!(matches!(
            window.controls(WindowControlsStyle::Standard),
            WindowControlSkin::Buttons {
                minus_icon_size: 11.0,
                maximize_icon_size: 10.0,
                close_icon_size: 11.0,
                gap: 12.0,
                padding: 12.0,
            }
        ));
        assert!(matches!(
            window.controls(WindowControlsStyle::Compact),
            WindowControlSkin::Buttons {
                minus_icon_size: 10.0,
                maximize_icon_size: 9.0,
                close_icon_size: 10.0,
                gap: 10.0,
                padding: 10.0,
            }
        ));
        assert!(matches!(
            window.controls(WindowControlsStyle::CloseWide),
            WindowControlSkin::Close {
                cell_size: 32.0,
                icon_size: 11.0,
                divider: Some((1.0, _)),
                ..
            }
        ));
        assert!(matches!(
            window.controls(WindowControlsStyle::CloseMicro),
            WindowControlSkin::Close {
                cell_size: 28.0,
                icon_size: 10.0,
                frame: None,
                divider: None,
            }
        ));
        assert!(matches!(
            window.controls(WindowControlsStyle::CloseFramed),
            WindowControlSkin::Close {
                cell_size: 22.0,
                icon_size: 10.0,
                frame: Some(_),
                divider: None,
            }
        ));
    }

    fn region_table(style: WindowControlsStyle) -> Vec<(WindowCommand, Rect)> {
        let program = ControlsProgram::new(style, builtin::skin());
        let (regions, count) = program.regions(Rect {
            h: 32.0,
            w: program.width(),
            x: 0.0,
            y: 0.0,
        });
        regions[..count]
            .iter()
            .map(|region| (region.command, region.bounds))
            .collect()
    }

    #[kithara::test]
    fn every_style_keeps_its_exact_interactive_regions() {
        assert_eq!(
            region_table(WindowControlsStyle::Standard),
            [
                (
                    WindowCommand::Minimize,
                    Rect {
                        h: 32.0,
                        w: 11.0,
                        x: 12.0,
                        y: 0.0,
                    },
                ),
                (
                    WindowCommand::ToggleMaximize,
                    Rect {
                        h: 32.0,
                        w: 10.0,
                        x: 35.0,
                        y: 0.0,
                    },
                ),
                (
                    WindowCommand::Close,
                    Rect {
                        h: 32.0,
                        w: 11.0,
                        x: 57.0,
                        y: 0.0,
                    },
                ),
            ]
        );
        assert_eq!(
            region_table(WindowControlsStyle::Compact),
            [
                (
                    WindowCommand::Minimize,
                    Rect {
                        h: 32.0,
                        w: 10.0,
                        x: 10.0,
                        y: 0.0,
                    },
                ),
                (
                    WindowCommand::ToggleMaximize,
                    Rect {
                        h: 32.0,
                        w: 9.0,
                        x: 30.0,
                        y: 0.0,
                    },
                ),
                (
                    WindowCommand::Close,
                    Rect {
                        h: 32.0,
                        w: 10.0,
                        x: 49.0,
                        y: 0.0,
                    },
                ),
            ]
        );
        for (style, size) in [
            (WindowControlsStyle::CloseWide, 32.0),
            (WindowControlsStyle::CloseMicro, 28.0),
            (WindowControlsStyle::CloseFramed, 22.0),
        ] {
            assert_eq!(
                region_table(style),
                [(
                    WindowCommand::Close,
                    Rect {
                        h: 32.0,
                        w: size,
                        x: 0.0,
                        y: 0.0,
                    },
                )],
                "{style:?}"
            );
        }
    }

    #[kithara::test]
    fn every_style_keeps_its_existing_layout_lengths() {
        for (style, width, height) in [
            (WindowControlsStyle::Standard, 80.0, Length::Fill),
            (WindowControlsStyle::Compact, 69.0, Length::Fill),
            (WindowControlsStyle::CloseWide, 32.0, Length::Fixed(32.0)),
            (WindowControlsStyle::CloseMicro, 28.0, Length::Fill),
            (WindowControlsStyle::CloseFramed, 22.0, Length::Fill),
        ] {
            let program = ControlsProgram::new(style, builtin::skin());
            assert_eq!(program.width(), width, "{style:?} width");
            assert_eq!(program.height(), height, "{style:?} height");
            assert_eq!(
                program.size(),
                Size::new(Length::Fixed(width), height),
                "{style:?} layer size"
            );
        }
    }

    #[kithara::test]
    fn hover_changes_only_the_glyph_under_the_pointer() {
        let skin = builtin::skin();
        let program = ControlsProgram::new(WindowControlsStyle::Standard, skin);
        let list = program.paint(
            Rect {
                h: 32.0,
                w: program.width(),
                x: 0.0,
                y: 0.0,
            },
            Some(Pt { x: 17.5, y: 16.0 }),
        );
        let colors = list
            .commands()
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Stroke { color, .. } => Some(*color),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(colors.len(), 4);
        assert_eq!(colors[0], skin.rgba(skin.window.icon_hover_color));
        assert!(
            colors[1..]
                .iter()
                .all(|color| *color == skin.rgba(skin.window.icon_color))
        );
    }

    #[kithara::test]
    fn close_styles_retain_their_frame_and_divider_commands() {
        let skin = builtin::skin();
        let framed = ControlsProgram::new(WindowControlsStyle::CloseFramed, skin);
        let framed_list = framed.paint(
            Rect {
                h: 22.0,
                w: framed.width(),
                x: 0.0,
                y: 0.0,
            },
            None,
        );
        let WindowControlSkin::Close {
            frame: Some(frame), ..
        } = skin.window.controls(WindowControlsStyle::CloseFramed)
        else {
            panic!("the close-framed style must carry a frame");
        };
        assert!(matches!(
            framed_list.commands().first(),
            Some(DrawCmd::Stroke {
                geom: Geom::Rect(_),
                color,
                pen,
            }) if *color == skin.rgba(frame.border) && pen.width == frame.border_width
        ));

        let wide = ControlsProgram::new(WindowControlsStyle::CloseWide, skin);
        let wide_list = wide.paint(
            Rect {
                h: 32.0,
                w: wide.width(),
                x: 0.0,
                y: 0.0,
            },
            None,
        );
        let WindowControlSkin::Close {
            divider: Some((divider_width, divider_role)),
            ..
        } = skin.window.controls(WindowControlsStyle::CloseWide)
        else {
            panic!("the close-wide style must carry a divider");
        };
        assert!(matches!(
            wide_list.commands().last(),
            Some(DrawCmd::Fill {
                geom: Geom::Rect(Rect { h: 32.0, w, x: 0.0, y: 0.0 }),
                paint: Paint::Solid(color),
            }) if *w == divider_width && *color == skin.rgba(divider_role)
        ));
    }

    #[kithara::test]
    fn layer_keeps_paint_local_and_hits_absolute() {
        let program = ControlsProgram::new(WindowControlsStyle::Standard, builtin::skin());
        let bounds = Rect {
            h: 32.0,
            w: program.width(),
            x: 100.0,
            y: 40.0,
        };
        let state = ControlsState::default();
        let pointer = Pt { x: 117.5, y: 56.0 };
        let layer = program.layer(&state, bounds, Some(pointer));

        assert_eq!(
            layer.draw(),
            &program.paint(
                Rect {
                    h: 32.0,
                    w: program.width(),
                    x: 0.0,
                    y: 0.0,
                },
                Some(Pt { x: 17.5, y: 16.0 }),
            )
        );
        assert_eq!(
            layer.hits()[0].area(),
            Rect {
                h: 32.0,
                w: 11.0,
                x: 112.0,
                y: 40.0,
            }
        );
        assert_eq!(layer.cursor_at(Some(pointer)), CursorShape::Pointer);
        assert_eq!(
            layer.cursor_at(Some(Pt { x: 129.0, y: 56.0 })),
            CursorShape::None,
            "the gap between buttons must not claim the pointer",
        );

        let hit_layer = program.hit_layer(&state, bounds);
        assert!(hit_layer.draw().commands().is_empty());
        assert_eq!(hit_layer.hits(), layer.hits());
    }

    #[kithara::test]
    fn a_gap_does_not_arm_or_capture() {
        let program = ControlsProgram::new(WindowControlsStyle::Standard, builtin::skin());
        let bounds = control_bounds(&program);
        let mut state = ControlsState::default();
        let pointer = absolute(bounds, Pt { x: 29.0, y: 16.0 });
        let layer = program.hit_layer(&state, bounds);
        let (outcome, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Down, None),
            &layer,
            Some(pointer),
        );

        assert_eq!(outcome, Outcome::IGNORED);
        assert!(!redraw);
        assert_eq!(state.armed, None);
    }

    fn control_bounds(program: &ControlsProgram) -> Rect {
        Rect {
            h: 32.0,
            w: program.width(),
            x: 100.0,
            y: 40.0,
        }
    }

    fn absolute(bounds: Rect, local: Pt) -> Pt {
        Pt {
            x: bounds.x + local.x,
            y: bounds.y + local.y,
        }
    }

    fn assert_release(program: &ControlsProgram, local: Pt, command: WindowCommand) {
        let bounds = control_bounds(program);
        let pointer = absolute(bounds, local);
        let mut state = ControlsState::default();
        let layer = program.hit_layer(&state, bounds);
        let (pressed, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Down, None),
            &layer,
            Some(pointer),
        );
        assert_eq!(
            pressed,
            Outcome::captured().with_ownership(crate::interact::PointerOwnership::Claim)
        );
        assert!(!redraw);
        assert_eq!(state.armed, Some(command));

        let (released, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Up, None),
            &layer,
            Some(pointer),
        );

        assert_eq!(
            released,
            Outcome::set(command).with_ownership(crate::interact::PointerOwnership::Release)
        );
        assert!(!redraw);
        assert_eq!(state.armed, None);
    }

    #[kithara::test]
    fn standard_buttons_and_close_only_controls_emit_their_own_commands() {
        let standard = ControlsProgram::new(WindowControlsStyle::Standard, builtin::skin());
        assert_release(&standard, Pt { x: 17.5, y: 16.0 }, WindowCommand::Minimize);
        assert_release(
            &standard,
            Pt { x: 40.0, y: 16.0 },
            WindowCommand::ToggleMaximize,
        );
        assert_release(&standard, Pt { x: 62.5, y: 16.0 }, WindowCommand::Close);

        let close = ControlsProgram::new(WindowControlsStyle::CloseFramed, builtin::skin());
        assert_release(&close, Pt { x: 11.0, y: 16.0 }, WindowCommand::Close);
    }

    #[kithara::test]
    fn leaving_a_window_button_before_release_cancels_its_command() {
        let program = ControlsProgram::new(WindowControlsStyle::Standard, builtin::skin());
        let bounds = control_bounds(&program);
        let mut state = ControlsState::default();
        let layer = program.hit_layer(&state, bounds);
        let pointer = absolute(bounds, Pt { x: 17.5, y: 16.0 });
        let (pressed, _) = program.update(
            &mut state,
            pointer_input(PointerPhase::Down, None),
            &layer,
            Some(pointer),
        );
        assert_eq!(
            pressed,
            Outcome::captured().with_ownership(crate::interact::PointerOwnership::Claim)
        );

        let outside = absolute(bounds, Pt { x: 90.0, y: 16.0 });
        let (released, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Up, None),
            &layer,
            Some(outside),
        );

        assert_eq!(
            released,
            Outcome::captured().with_ownership(crate::interact::PointerOwnership::Release)
        );
        assert!(!redraw);
        assert_eq!(state.armed, None);
    }

    #[kithara::test]
    fn hover_transitions_request_only_the_needed_repaints() {
        let program = ControlsProgram::new(WindowControlsStyle::Standard, builtin::skin());
        let bounds = control_bounds(&program);
        let mut state = ControlsState::default();
        let layer = program.hit_layer(&state, bounds);
        let minimize = absolute(bounds, Pt { x: 17.5, y: 16.0 });

        let (outcome, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Move, Some(minimize)),
            &layer,
            Some(minimize),
        );
        assert_eq!(outcome, Outcome::IGNORED);
        assert!(redraw);
        assert_eq!(state.hovered, Some(WindowCommand::Minimize));

        let (_, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Move, Some(minimize)),
            &layer,
            Some(minimize),
        );
        assert!(!redraw);

        let (_, redraw) = program.update(
            &mut state,
            pointer_input(PointerPhase::Leave, None),
            &layer,
            None,
        );
        assert!(redraw);
        assert_eq!(state.hovered, None);
    }
}
