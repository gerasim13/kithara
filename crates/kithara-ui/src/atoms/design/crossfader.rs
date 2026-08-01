use iced::{
    Color, Element, Event, Length, Padding, Point, Rectangle, Renderer, Size, Theme,
    alignment::{Horizontal, Vertical},
    mouse::{self, Cursor},
    widget::{
        Column, Row, Space,
        canvas::{self, Action, Canvas, Frame, Geometry, Path, Stroke},
        container,
    },
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use crate::{
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, replay},
    interact::{
        CursorShape, Hover, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track},
    },
    render::{Icon, ReadValue, Skin, UiEvent, fonts, scalar, shaped_text},
    skin::{FrameSkin, TickSkin},
    text::TextResources,
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Crossfader<'path, 'value, 'data, 'skin> {
    path: &'path str,
    ticks: bool,
    value: Option<&'value ReadValue<'data>>,
    skin: &'skin Skin,
}

impl<'a> Widget<'a> for Crossfader<'_, '_, '_, 'a> {
    fn view(self) -> Element<'a, UiEvent> {
        self.render(true)
    }
}

impl<'a> Crossfader<'_, '_, '_, 'a> {
    pub(crate) fn painted(self) -> Element<'a, UiEvent> {
        self.render(false)
    }

    fn render(self, interactive: bool) -> Element<'a, UiEvent> {
        let Some(ReadValue::Scalar(value)) = self.value else {
            return Space::new().into();
        };
        let metrics = &self.skin.crossfader;
        let letter = |content: &'a str| {
            shaped_text(content)
                .font(fonts::mono(metrics.letter_text.weight))
                .size(metrics.letter_text.size)
                .color(self.skin.color(metrics.letter_color))
        };
        let arrow =
            |icon: Icon| icon.view(metrics.arrow_size, self.skin.color(metrics.arrow_color));
        let side = |children: [Element<'a, UiEvent>; 2], alignment| {
            container(
                Row::with_children(children)
                    .spacing(metrics.arrow_gap)
                    .align_y(Vertical::Center),
            )
            .width(Length::Fill)
            .align_x(alignment)
            .into()
        };
        let labels = Row::with_children([
            side(
                [
                    letter(&metrics.left_label).into(),
                    arrow(Icon::ChevronsLeft),
                ],
                Horizontal::Left,
            ),
            container(
                shaped_text(&metrics.center_label)
                    .font(fonts::mono(metrics.label_text.weight))
                    .size(metrics.label_text.size)
                    .color(self.skin.color(metrics.label_color)),
            )
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .into(),
            side(
                [
                    arrow(Icon::ChevronsRight),
                    letter(&metrics.right_label).into(),
                ],
                Horizontal::Right,
            ),
        ])
        .width(Length::Fill);
        let ticks = self
            .ticks
            .then(|| TickRail::new(TickAxis::Horizontal, metrics.ticks, self.skin));
        let slider_height = ticks.as_ref().map_or(0.0, TickRail::reserved) + metrics.thumb_height;
        let paint = CrossfaderPaint {
            rail_background: self.skin.color(metrics.rail_background),
            rail_color: self.skin.color(metrics.rail_frame.border),
            rail_frame: metrics.rail_frame,
            rail_height: metrics.rail_height,
            thumb_color: self.skin.color(metrics.thumb_color),
            thumb_height: metrics.thumb_height,
            thumb_width: metrics.thumb_width,
            thumb_notch_color: self.skin.color(metrics.thumb_notch_color),
            thumb_notch_height: metrics.thumb_notch_height,
            thumb_notch_width: metrics.thumb_notch_width,
            ticks,
            text_resources: self.skin.text_resources(),
            value: value.clamp(0.0, 1.0).as_(),
        };
        let slider: Element<'a, UiEvent> = if interactive {
            Canvas::new(CrossfaderCanvas {
                drag: Scalar::builder()
                    .track(Track::AbsoluteHorizontal)
                    .hover(Hover::new(CursorShape::ResizeH))
                    .build(),
                path: self.path.to_owned(),
                paint,
            })
            .width(Length::Fill)
            .height(Length::Fixed(slider_height))
            .into()
        } else {
            Canvas::new(paint)
                .width(Length::Fill)
                .height(Length::Fixed(slider_height))
                .into()
        };

        container(
            Column::new()
                .push(slider)
                .push(labels)
                .spacing(metrics.label_gap)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .padding(
            Padding::ZERO
                .top(metrics.padding_top)
                .bottom(metrics.padding_bottom)
                .left(metrics.padding_x)
                .right(metrics.padding_x),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct CrossfaderCanvas<'skin> {
    drag: Scalar,
    path: String,
    paint: CrossfaderPaint<'skin>,
}

struct CrossfaderPaint<'skin> {
    rail_background: Color,
    rail_color: Color,
    rail_frame: FrameSkin,
    rail_height: f32,
    thumb_color: Color,
    thumb_height: f32,
    thumb_width: f32,
    thumb_notch_color: Color,
    thumb_notch_height: f32,
    thumb_notch_width: f32,
    ticks: Option<TickRail>,
    text_resources: &'skin TextResources,
    value: f32,
}

#[derive(Clone, Copy)]
pub(crate) enum TickAxis {
    Horizontal,
    Vertical,
}

pub(crate) struct TickRail {
    axis: TickAxis,
    metrics: TickSkin,
    color: Color,
    center_color: Color,
}

impl TickRail {
    pub(crate) fn new(axis: TickAxis, metrics: TickSkin, skin: &Skin) -> Self {
        Self {
            axis,
            metrics,
            color: skin.color(metrics.color),
            center_color: skin.color(metrics.center_color),
        }
    }

    fn last(&self) -> Option<usize> {
        self.metrics.count.checked_sub(1).filter(|last| *last > 0)
    }

    pub(crate) fn extent(&self) -> f32 {
        self.metrics.center_length.max(self.metrics.length)
    }

    pub(crate) fn reserved(&self) -> f32 {
        self.last()
            .map_or(0.0, |_| self.extent() + self.metrics.gap)
    }

    pub(crate) fn paint(&self, builder: &mut DrawListBuilder, rail: Rect) {
        let Some(last) = self.last() else {
            return;
        };
        let (span, cross) = match self.axis {
            TickAxis::Horizontal => (rail.w, rail.h),
            TickAxis::Vertical => (rail.h, rail.w),
        };
        let cross = cross.max(0.0);
        let travel = (span - self.metrics.inset * 2.0 - self.metrics.thickness).max(0.0);
        let center = last / 2;
        let steps: f32 = last.as_();
        for index in 0..self.metrics.count {
            let is_center = self.metrics.count % 2 == 1 && index == center;
            let length = if is_center {
                self.metrics.center_length
            } else {
                self.metrics.length
            }
            .min(cross);
            let step: f32 = index.as_();
            let offset = self.metrics.inset + step / steps * travel;
            let color = if is_center {
                self.center_color
            } else {
                self.color
            };
            let tick = match self.axis {
                TickAxis::Horizontal => Rect {
                    h: length,
                    w: self.metrics.thickness,
                    x: rail.x + offset,
                    y: rail.y + cross - length,
                },
                TickAxis::Vertical => Rect {
                    h: self.metrics.thickness,
                    w: length,
                    x: rail.x + cross - length,
                    y: rail.y + offset,
                },
            };
            builder.fill_rect(tick, color.into());
        }
    }
}

impl canvas::Program<UiEvent> for CrossfaderCanvas<'_> {
    type State = ScalarState;

    fn draw(
        &self,
        _state: &ScalarState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.draw(&(), renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &ScalarState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        self.drag
            .cursor(state, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        state: &mut ScalarState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        scalar(
            &self.path,
            self.drag.on_input(state, input, &hit, Instant::now()),
        )
    }
}

impl canvas::Program<UiEvent> for CrossfaderPaint<'_> {
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
        let mut builder = DrawListBuilder::default();
        let reserved = self.ticks.as_ref().map_or(0.0, |ticks| {
            ticks.paint(
                &mut builder,
                Rect {
                    h: ticks.extent(),
                    w: bounds.width,
                    x: 0.0,
                    y: 0.0,
                },
            );
            ticks.reserved()
        });
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        let track_height = (bounds.height - reserved).max(0.0);
        let rail_height = self.rail_height.min(track_height).max(0.0);
        let rail_point = Point::new(0.0, reserved + (track_height - rail_height) / 2.0);
        let rail_size = Size::new(bounds.width, rail_height);
        let rail = Path::rounded_rectangle(rail_point, rail_size, self.rail_frame.radius.into());
        frame.fill(&rail, self.rail_background);
        frame.stroke(
            &rail,
            Stroke::default()
                .with_color(self.rail_color)
                .with_width(self.rail_frame.border_width),
        );

        let thumb_width = self.thumb_width.min(bounds.width).max(0.0);
        let thumb_height = self.thumb_height.min(track_height).max(0.0);
        let travel = (bounds.width - thumb_width).max(0.0);
        let thumb_x = (self.value * travel).round();
        let thumb_y = reserved + (track_height - thumb_height) / 2.0;
        frame.fill_rectangle(
            Point::new(thumb_x, thumb_y),
            Size::new(thumb_width, thumb_height),
            self.thumb_color,
        );
        let notch_height = self.thumb_notch_height.min(thumb_height);
        if notch_height > 0.0 && self.thumb_notch_width > 0.0 {
            frame.fill_rectangle(
                Point::new(
                    thumb_x + (thumb_width - self.thumb_notch_width) / 2.0,
                    thumb_y + (thumb_height - notch_height) / 2.0,
                ),
                Size::new(self.thumb_notch_width, notch_height),
                self.thumb_notch_color,
            );
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom},
        skin::ColorRole,
    };

    fn rail(axis: TickAxis, count: usize) -> TickRail {
        TickRail {
            axis,
            metrics: TickSkin {
                count,
                thickness: 2.0,
                length: 6.0,
                center_length: 10.0,
                gap: 4.0,
                inset: 5.0,
                color: ColorRole::LineDim,
                center_color: ColorRole::LineDim,
            },
            color: builtin::skin().color(ColorRole::LineDim),
            center_color: builtin::skin().color(ColorRole::AccentSoft),
        }
    }

    fn rects(rail: &TickRail, area: Rect) -> Vec<Rect> {
        let mut builder = DrawListBuilder::default();
        rail.paint(&mut builder, area);
        builder
            .finish()
            .commands()
            .iter()
            .map(|command| match command {
                DrawCmd::Fill {
                    geom: Geom::Rect(rect),
                    ..
                } => *rect,
                other => panic!("a tick rail draws rectangles and nothing else, got {other:?}"),
            })
            .collect()
    }

    #[kithara::test]
    fn ticks_span_the_rail_from_inset_to_inset() {
        // 100 wide, 5 px inset each end, 2 px thick: 88 px of travel over 4 gaps.
        let ticks = rects(
            &rail(TickAxis::Horizontal, 5),
            Rect {
                h: 12.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
        );

        assert_eq!(ticks.len(), 5);
        assert_eq!(ticks[0].x, 5.0);
        assert_eq!(ticks[4].x, 93.0, "the last tick ends at the far inset");
        assert_eq!(ticks[1].x - ticks[0].x, 22.0, "the step is even");
    }

    #[kithara::test]
    fn the_middle_tick_is_longer_only_when_there_is_a_middle() {
        let odd = rects(
            &rail(TickAxis::Horizontal, 5),
            Rect {
                h: 12.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
        );
        let even = rects(
            &rail(TickAxis::Horizontal, 4),
            Rect {
                h: 12.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
        );

        assert_eq!(odd[2].h, 10.0, "an odd count has a centre tick");
        assert!(
            odd.iter()
                .enumerate()
                .all(|(index, tick)| index == 2 || (tick.h - 6.0).abs() < f32::EPSILON)
        );
        assert!(
            even.iter().all(|tick| (tick.h - 6.0).abs() < f32::EPSILON),
            "an even count has no middle to mark"
        );
    }

    #[kithara::test]
    fn a_tick_never_outgrows_the_rail_it_sits_in() {
        let squashed = rects(
            &rail(TickAxis::Horizontal, 5),
            Rect {
                h: 3.0,
                w: 100.0,
                x: 0.0,
                y: 0.0,
            },
        );

        assert!(squashed.iter().all(|tick| tick.h <= 3.0));
        assert!(
            squashed.iter().all(|tick| tick.y >= 0.0),
            "clamping the length must not push a tick above the rail"
        );
    }

    #[kithara::test]
    fn the_vertical_axis_swaps_the_span_and_the_cross() {
        let ticks = rects(
            &rail(TickAxis::Vertical, 5),
            Rect {
                h: 100.0,
                w: 12.0,
                x: 0.0,
                y: 0.0,
            },
        );

        assert_eq!(ticks[0].y, 5.0);
        assert_eq!(ticks[4].y, 93.0);
        assert_eq!(ticks[0].h, 2.0, "thickness runs across the span");
        assert_eq!(ticks[0].w, 6.0, "length runs across the rail");
    }
}
