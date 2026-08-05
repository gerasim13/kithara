use iced::{
    Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme,
    alignment::{Horizontal, Vertical},
    mouse::{self, Cursor},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry, Path, Stroke},
    },
};
use num_traits::ToPrimitive;

use crate::{
    interact::{iced as iced_interact, recognizers::click},
    render::{IcedSkin, ReadValue, Skin, UiEvent, fonts, index},
    skin::SegmentedSkin,
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Segmented<'a, 'value, 'data, 'skin> {
    skin: &'skin Skin,
    path: &'a str,
    value: Option<&'value ReadValue<'data>>,
    items: Vec<&'a str>,
}

impl<'a> Widget<'a> for Segmented<'a, '_, '_, '_> {
    fn view(self) -> Element<'a, UiEvent> {
        let path = self.path.to_owned();
        let Some(paint) = self.paint() else {
            return Space::new().into();
        };
        Canvas::new(SegmentedCanvas { paint, path })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<'a, 'value, 'data, 'skin> Segmented<'a, 'value, 'data, 'skin> {
    fn paint(self) -> Option<SegmentedPaint<'a>> {
        let Some(ReadValue::Scalar(value)) = self.value else {
            return None;
        };
        let active = value
            .round()
            .to_usize()
            .filter(|index| *index < self.items.len());
        Some(SegmentedPaint {
            active,
            items: self.items,
            metrics: self.skin.segmented,
            colors: SegmentedColors {
                background: self.skin.color(self.skin.segmented.background),
                active_background: self.skin.color(self.skin.segmented.active_background),
                active_text: self.skin.color(self.skin.segmented.active_text),
                inactive_text: self.skin.color(self.skin.segmented.inactive_text),
                frame: self.skin.color(self.skin.segmented.frame.border),
            },
        })
    }

    pub(crate) fn painted(self) -> Element<'a, UiEvent> {
        let Some(paint) = self.paint() else {
            return Space::new().into();
        };
        Canvas::new(paint)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

struct SegmentedCanvas<'a> {
    paint: SegmentedPaint<'a>,
    path: String,
}

struct SegmentedPaint<'a> {
    active: Option<usize>,
    colors: SegmentedColors,
    metrics: SegmentedSkin,
    items: Vec<&'a str>,
}

#[derive(Clone, Copy)]
struct SegmentedColors {
    active_background: Color,
    active_text: Color,
    background: Color,
    frame: Color,
    inactive_text: Color,
}

impl canvas::Program<UiEvent> for SegmentedCanvas<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.draw(&(), renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if self.paint.items.is_empty() || !cursor.is_over(bounds) {
            mouse::Interaction::default()
        } else {
            mouse::Interaction::Pointer
        }
    }

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        let selected = hit.uniform_horizontal_index(self.paint.items.len())?;
        index(&self.path, click::on_input(input, &hit).map(|()| selected))
    }
}

impl canvas::Program<UiEvent> for SegmentedPaint<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let Some(cell_width) = cell_width(bounds, self.items.len()) else {
            return vec![frame.into_geometry()];
        };
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), self.colors.background);
        if let Some(active) = self.active {
            let x = index_position(active, cell_width);
            frame.fill_rectangle(
                Point::new(x, 0.0),
                Size::new(cell_width, bounds.height),
                self.colors.active_background,
            );
        }
        draw_grid(
            &mut frame,
            bounds,
            cell_width,
            self.items.len(),
            self.metrics,
            self.colors.frame,
        );
        for (index, item) in self.items.iter().enumerate() {
            let color = if self.active == Some(index) {
                self.colors.active_text
            } else {
                self.colors.inactive_text
            };
            frame.fill_text(canvas::Text {
                content: (*item).to_owned(),
                position: Point::new(
                    index_position(index, cell_width) + cell_width / 2.0,
                    bounds.height / 2.0,
                ),
                max_width: (cell_width - self.metrics.padding_x * 2.0).max(0.0),
                color,
                size: self.metrics.text.size.into(),
                font: fonts::mono(self.metrics.text.weight),
                align_x: Horizontal::Center.into(),
                align_y: Vertical::Center,
                shaping: iced::widget::text::Shaping::Advanced,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

fn cell_width(bounds: Rectangle, count: usize) -> Option<f32> {
    let count = count.to_f32()?;
    (count > 0.0 && bounds.width > 0.0).then_some(bounds.width / count)
}

fn index_position(index: usize, cell_width: f32) -> f32 {
    index.to_f32().map_or(0.0, |index| index * cell_width)
}

fn draw_grid(
    frame: &mut Frame,
    bounds: Rectangle,
    cell_width: f32,
    count: usize,
    metrics: SegmentedSkin,
    color: Color,
) {
    let width = metrics.frame.border_width;
    if width <= 0.0 {
        return;
    }
    let inset = width / 2.0;
    let frame_path = Path::rounded_rectangle(
        Point::new(inset, inset),
        Size::new(
            (bounds.width - width).max(0.0),
            (bounds.height - width).max(0.0),
        ),
        metrics.frame.radius.into(),
    );
    frame.stroke(
        &frame_path,
        Stroke::default().with_color(color).with_width(width),
    );
    for index in 1..count {
        let x = index_position(index, cell_width);
        frame.stroke(
            &Path::line(Point::new(x, 0.0), Point::new(x, bounds.height)),
            Stroke::default().with_color(color).with_width(width),
        );
    }
}

#[cfg(test)]
mod tests {
    use iced::{event, mouse::Button, window::RedrawRequest};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, render::ControlAction};

    fn paint() -> SegmentedPaint<'static> {
        let skin = builtin::skin();
        SegmentedPaint {
            items: vec!["MICRO", "1", "2", "4"],
            active: Some(1),
            metrics: skin.segmented,
            colors: SegmentedColors {
                background: skin.color(skin.segmented.background),
                active_background: skin.color(skin.segmented.active_background),
                active_text: skin.color(skin.segmented.active_text),
                inactive_text: skin.color(skin.segmented.inactive_text),
                frame: skin.color(skin.segmented.frame.border),
            },
        }
    }

    #[kithara::test]
    fn the_leaf_segmented_publishes_its_own_path_and_index() {
        let program = SegmentedCanvas {
            path: "cells/beat".to_owned(),
            paint: paint(),
        };
        let bounds = Rectangle::new(Point::new(10.0, 20.0), Size::new(220.0, 26.0));
        let cursor = Cursor::Available(Point::new(147.5, 33.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));

        let action = canvas::Program::update(&program, &mut (), &press, bounds, cursor)
            .unwrap_or_else(|| panic!("a segmented press inside its bounds must publish"));

        assert_eq!(
            action.into_inner(),
            (
                Some(UiEvent::Control {
                    path: "cells/beat".to_owned(),
                    action: ControlAction::SelectIndex(2),
                }),
                RedrawRequest::Wait,
                event::Status::Captured,
            )
        );
    }

    #[kithara::test]
    fn the_paint_only_segmented_has_no_leaf_input() {
        let paint = paint();
        let bounds = Rectangle::new(Point::new(10.0, 20.0), Size::new(220.0, 26.0));
        let cursor = Cursor::Available(Point::new(147.5, 33.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));

        assert!(
            canvas::Program::<UiEvent>::update(&paint, &mut (), &press, bounds, cursor).is_none()
        );
        assert_eq!(
            canvas::Program::<UiEvent>::mouse_interaction(&paint, &(), bounds, cursor),
            mouse::Interaction::None
        );
    }
}
