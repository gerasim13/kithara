use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{self, Cursor},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry},
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
    render::{ReadValue, Skin, StereoLevels, UiEvent, scalar, theme::RenderPalette},
    skin::VuStereoSkin,
    text::TextResources,
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct StereoMeter<'path, 'value, 'data, 'skin> {
    path: &'path str,
    value: Option<&'value ReadValue<'data>>,
    skin: &'skin Skin,
}

impl<'a> Widget<'a> for StereoMeter<'_, '_, '_, 'a> {
    fn view(self) -> Element<'a, UiEvent> {
        let Some(ReadValue::Stereo(levels)) = self.value else {
            return Space::new().into();
        };
        Canvas::new(StereoMeterCanvas {
            drag: Scalar::builder()
                .track(Track::AbsoluteHorizontal)
                .hover(Hover::new(CursorShape::ResizeH))
                .build(),
            path: self.path.to_owned(),
            metrics: self.skin.vu_stereo,
            levels: *levels,
            palette: self.skin.palette,
            text_resources: self.skin.text_resources(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct StereoMeterCanvas<'skin> {
    drag: Scalar,
    path: String,
    metrics: VuStereoSkin,
    levels: StereoLevels,
    palette: RenderPalette,
    text_resources: &'skin TextResources,
}

impl canvas::Program<UiEvent> for StereoMeterCanvas<'_> {
    type State = ScalarState;

    fn draw(
        &self,
        _state: &ScalarState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mut builder = DrawListBuilder::default();
        builder.fill_rect(
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
            self.palette.bg_deep.into(),
        );

        for (level, y) in [self.levels.l, self.levels.r]
            .into_iter()
            .zip([self.metrics.channel_l_y, self.metrics.channel_r_y])
        {
            draw_channel(&mut builder, y, level, self.metrics, self.palette);
        }

        let x = self.levels.volume.clamp(0.0, 1.0) * bounds.width;
        builder.fill_rect(
            Rect {
                h: bounds.height,
                w: self.metrics.carriage_width,
                x,
                y: 0.0,
            },
            self.palette.accent.into(),
        );
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
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

fn draw_channel(
    builder: &mut DrawListBuilder,
    y: f32,
    level: f32,
    metrics: VuStereoSkin,
    palette: RenderPalette,
) {
    let count: f32 = metrics.segment_count.as_();
    let lit = (level.clamp(0.0, 1.0) * count).round();

    for index in 0..metrics.segment_count {
        let index: f32 = index.as_();
        let x = index * (metrics.segment_width + metrics.segment_gap);
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
        builder.fill_rect(
            Rect {
                h: metrics.segment_height,
                w: metrics.segment_width,
                x,
                y,
            },
            color.into(),
        );
    }
}
