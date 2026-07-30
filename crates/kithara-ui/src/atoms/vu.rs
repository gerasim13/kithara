use iced::{
    Color, Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{self, Cursor},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry},
    },
};
use num_traits::cast::AsPrimitive;

use crate::{
    atoms::design::crossfader::{TickAxis, TickRail},
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, replay},
    render::{ReadValue, Skin, StereoLevels, UiEvent, theme::RenderPalette},
    skin::VuVerticalSkin,
    text::TextResources,
    widgets::{
        Widget,
        behavior::{HoverState, ScalarDrag, ScalarDragMode, ScalarDragState},
    },
};

#[derive(bon::Builder)]
pub(crate) struct VerticalVu<'path, 'value, 'data, 'skin> {
    path: &'path str,
    ticks: bool,
    value: Option<&'value ReadValue<'data>>,
    skin: &'skin Skin,
}

impl<'a> Widget<'a> for VerticalVu<'_, '_, '_, 'a> {
    fn view(self) -> Element<'a, UiEvent> {
        let Some(ReadValue::Stereo(levels)) = self.value else {
            return Space::new().into();
        };
        Canvas::new(VerticalVuCanvas {
            drag: ScalarDrag::builder()
                .path(self.path.to_owned())
                .mode(ScalarDragMode::Vertical)
                .hover(HoverState::new(mouse::Interaction::ResizingVertically))
                .build(),
            metrics: self.skin.vu_vertical,
            ticks: self
                .ticks
                .then(|| TickRail::new(TickAxis::Vertical, self.skin.vu_vertical.ticks, self.skin)),
            levels: *levels,
            palette: self.skin.palette,
            thumb_color: self.skin.color(self.skin.vu_vertical.thumb_color),
            thumb_notch_color: self.skin.color(self.skin.vu_vertical.thumb_notch_color),
            text_resources: self.skin.text_resources(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct VerticalVuCanvas<'skin> {
    drag: ScalarDrag,
    metrics: VuVerticalSkin,
    ticks: Option<TickRail>,
    levels: StereoLevels,
    palette: RenderPalette,
    thumb_color: Color,
    thumb_notch_color: Color,
    text_resources: &'skin TextResources,
}

impl canvas::Program<UiEvent> for VerticalVuCanvas<'_> {
    type State = ScalarDragState;

    fn draw(
        &self,
        _state: &ScalarDragState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let canvas_bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            ..bounds
        };
        let fader = if self.ticks.is_some() {
            fader_bounds(canvas_bounds, self.metrics.fader_width)
        } else {
            canvas_bounds
        };
        if let Some(rail) = &self.ticks {
            rail.draw(
                &mut frame,
                tick_rail_bounds(canvas_bounds, fader.x, self.metrics.ticks.gap),
            );
        }
        let mut builder = DrawListBuilder::default();
        draw_segments(&mut builder, fader, self.levels, self.metrics, self.palette);
        draw_thumb(
            &mut builder,
            fader,
            self.levels.volume,
            self.metrics,
            self.thumb_color,
            self.thumb_notch_color,
        );
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
    }

    delegate::delegate! {
        to self.drag {
            fn update(
                &self,
                state: &mut ScalarDragState,
                event: &Event,
                bounds: Rectangle,
                cursor: Cursor,
            ) -> Option<Action<UiEvent>>;
            fn mouse_interaction(
                &self,
                state: &ScalarDragState,
                bounds: Rectangle,
                cursor: Cursor,
            ) -> mouse::Interaction;
        }
    }
}

fn fader_bounds(bounds: Rectangle, width: f32) -> Rectangle {
    let width = width.clamp(0.0, bounds.width);
    Rectangle {
        x: bounds.x + bounds.width - width,
        width,
        ..bounds
    }
}

fn tick_rail_bounds(bounds: Rectangle, fader_x: f32, gap: f32) -> Rectangle {
    let right = (fader_x - gap).max(bounds.x);
    Rectangle {
        width: right - bounds.x,
        ..bounds
    }
}

fn draw_segments(
    builder: &mut DrawListBuilder,
    bounds: Rectangle,
    levels: StereoLevels,
    metrics: VuVerticalSkin,
    palette: RenderPalette,
) {
    let step = metrics.segment_height + metrics.segment_gap;
    let count = ((bounds.height + metrics.segment_gap) / step).floor();
    if count <= 0.0 {
        return;
    }

    let count_usize: usize = count.as_();
    let level = (levels.l.max(levels.r) * levels.volume).clamp(0.0, 1.0);
    let lit = (level * count).round();
    let width = (bounds.width - metrics.segment_inset_x * 2.0).max(0.0);
    for index in 0..count_usize {
        let index: f32 = index.as_();
        let ratio = index / count;
        let color = if index >= lit {
            palette.bg_inset
        } else if ratio > metrics.danger_threshold {
            palette.danger
        } else if ratio > metrics.warning_threshold {
            palette.warning
        } else {
            palette.success
        };
        let y = bounds.y + bounds.height - metrics.segment_height - index * step;
        builder.fill_rect(
            Rect {
                h: metrics.segment_height,
                w: width,
                x: bounds.x + metrics.segment_inset_x,
                y,
            },
            color.into(),
        );
    }
}

fn draw_thumb(
    builder: &mut DrawListBuilder,
    bounds: Rectangle,
    volume: f32,
    metrics: VuVerticalSkin,
    color: Color,
    notch_color: Color,
) {
    let travel = (bounds.height - metrics.thumb_height).max(0.0);
    let y = bounds.y + ((1.0 - volume.clamp(0.0, 1.0)) * travel).round();
    builder.fill_rect(
        Rect {
            h: metrics.thumb_height,
            w: bounds.width,
            x: bounds.x,
            y,
        },
        color.into(),
    );
    if metrics.thumb_notch_offset < metrics.thumb_height {
        builder.fill_rect(
            Rect {
                h: metrics.thumb_notch_height,
                w: bounds.width,
                x: bounds.x,
                y: y + metrics.thumb_notch_offset,
            },
            notch_color.into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn the_fader_keeps_its_width_and_yields_the_rest_to_the_ticks() {
        let bare = fader_bounds(
            Rectangle {
                x: 3.0,
                width: 18.0,
                ..Rectangle::default()
            },
            18.0,
        );
        let with_ticks = fader_bounds(
            Rectangle {
                x: 3.0,
                width: 38.0,
                ..Rectangle::default()
            },
            18.0,
        );

        assert_eq!(bare.x, 3.0);
        assert_eq!(bare.width, 18.0);
        assert_eq!(with_ticks.x, 23.0);
        assert_eq!(with_ticks.width, 18.0);
    }
}
