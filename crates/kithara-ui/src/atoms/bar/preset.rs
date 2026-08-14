use num_traits::ToPrimitive;

use crate::{
    atoms::{
        button::VisualState,
        design::quad::{border, quad},
        painter::IndexedVisual,
    },
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::Skin,
    skin::{ColorRole, FontFamily, GlobalBarSkin, TextRoleSkin},
    solve::{Length, Size},
    text::TextContext,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PresetItem {
    pub(crate) label: &'static str,
    pub(crate) name: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PresetData {
    pub(crate) active: Option<usize>,
    pub(crate) items: &'static [PresetItem],
}

pub(crate) struct Preset {
    active: Rgba,
    active_hovered: Rgba,
    active_text: Rgba,
    chip_stroke: Rgba,
    hovered: Rgba,
    idle: Rgba,
    inactive_text: Rgba,
    metrics: GlobalBarSkin,
    panel: Rgba,
    pressed: Rgba,
    role: TextRoleSkin,
    selector_fill: Rgba,
    selector_stroke: Rgba,
}

struct ChipFace {
    active: bool,
    bounds: Rect,
    index: usize,
    visual: IndexedVisual,
}

impl Preset {
    pub(crate) fn new(skin: &Skin) -> Self {
        let metrics = skin.global_bar;
        Self {
            active: skin.palette.accent,
            active_hovered: skin.palette.accent_strong,
            active_text: skin.palette.bg,
            chip_stroke: skin.rgba(metrics.chip_frame.border),
            hovered: skin.palette.bg_panel_2,
            idle: skin.palette.bg_panel,
            inactive_text: skin.palette.text_dim,
            metrics,
            panel: skin.palette.bg_panel,
            pressed: skin.palette.accent_soft,
            role: TextRoleSkin {
                color: ColorRole::TextDim,
                font: FontFamily::Mono,
                size: metrics.chip_text.size,
                spacing: 0.0,
                weight: metrics.chip_text.weight,
            },
            selector_fill: skin.palette.line,
            selector_stroke: skin.rgba(metrics.selector_frame.border),
        }
    }

    pub(crate) fn declared(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(self.metrics.selector_width),
            Length::Fixed(self.metrics.height),
        )
    }

    pub(crate) fn selector(&self, bounds: Rect) -> Rect {
        Rect {
            h: (bounds.h - self.metrics.selector_padding_y * 2.0).max(0.0),
            w: (bounds.w - self.metrics.selector_padding_x * 2.0).max(0.0),
            x: bounds.x + self.metrics.selector_padding_x,
            y: bounds.y + self.metrics.selector_padding_y,
        }
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &PresetData,
        bounds: Rect,
        visual: IndexedVisual,
    ) {
        list.fill_rect(bounds, self.panel);
        let selector = self.selector(bounds);
        list.fill_rounded_rect(
            selector,
            self.metrics.selector_frame.radius,
            self.selector_fill,
        );
        let Some(width) = self.chip_width(selector.w, data.items.len()) else {
            return;
        };
        let mut x = selector.x;
        for (index, item) in data.items.iter().enumerate() {
            let chip = Self::chip(selector, width, x);
            self.paint_chip(
                list,
                text,
                item,
                &ChipFace {
                    active: data.active == Some(index),
                    bounds: chip,
                    index,
                    visual,
                },
            );
            x += width + self.metrics.chip_gap;
        }
        border(
            list,
            selector,
            self.metrics.selector_frame,
            self.selector_stroke,
        );
    }

    fn chip_width(&self, width: f32, count: usize) -> Option<f32> {
        let count = count.to_f32()?;
        let gaps = self.metrics.chip_gap * (count - 1.0).max(0.0);
        (count > 0.0 && width > gaps).then_some((width - gaps) / count)
    }

    pub(crate) fn hit_index(&self, data: &PresetData, bounds: Rect, point: Pt) -> Option<usize> {
        let selector = self.selector(bounds);
        let width = self.chip_width(selector.w, data.items.len())?;
        let mut x = selector.x;
        for index in 0..data.items.len() {
            if Self::chip(selector, width, x).contains(point) {
                return Some(index);
            }
            x += width + self.metrics.chip_gap;
        }
        None
    }

    const fn chip(selector: Rect, width: f32, x: f32) -> Rect {
        Rect {
            x,
            w: width,
            ..selector
        }
    }

    fn paint_chip(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        item: &PresetItem,
        face: &ChipFace,
    ) {
        let state = cell_state(face.visual, face.index);
        let fill = match (face.active, state) {
            (true, VisualState::Hovered) => self.active_hovered,
            (_, VisualState::Pressed) => self.pressed,
            (true, VisualState::Idle) => self.active,
            (false, VisualState::Hovered) => self.hovered,
            (false, VisualState::Idle) => self.idle,
        };
        quad(
            list,
            face.bounds,
            self.metrics.chip_frame,
            fill,
            self.chip_stroke,
        );
        let run = text.shape(item.label, self.role, None);
        let content = Rect {
            h: (face.bounds.h - self.metrics.chip_padding_y * 2.0).max(0.0),
            w: (face.bounds.w - self.metrics.chip_padding_x * 2.0).max(0.0),
            x: face.bounds.x + self.metrics.chip_padding_x,
            y: face.bounds.y + self.metrics.chip_padding_y,
        };
        let mut word = list.child();
        word.text(
            &run,
            item.label,
            Transform::translate(Pt {
                x: content.x + (content.w - run.width()) / 2.0,
                y: content.y + (content.h - run.height()) / 2.0,
            }),
            if face.active {
                self.active_text
            } else {
                self.inactive_text
            },
        );
        list.clip(content, word.finish());
    }
}

fn cell_state(visual: IndexedVisual, index: usize) -> VisualState {
    if visual.hovered == Some(index) && visual.pressed_origin == Some(index) {
        VisualState::Pressed
    } else if visual.hovered == Some(index) {
        VisualState::Hovered
    } else {
        VisualState::Idle
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{IndexedVisual, Preset, PresetData};
    use crate::{
        builtin,
        draw::{DrawCmd, DrawList, DrawListBuilder, Geom, Paint, Rect},
        mount,
        render::{ReadValue, Reads},
        skin::{ColorRole, FontFamily, TextRoleSkin},
        solve::{Length, Size},
        text::TextContext,
    };

    const BOUNDS: Rect = Rect {
        h: 42.0,
        w: 126.0,
        x: 4.0,
        y: 6.0,
    };

    struct NoReads;

    impl Reads for NoReads {
        fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
            None
        }
    }

    fn data(active: Option<usize>) -> PresetData {
        let mut data = mount::Preset::snapshot(&NoReads);
        data.active = active;
        data
    }

    fn draw(active: Option<usize>, visual: IndexedVisual) -> DrawList {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let mut list = DrawListBuilder::default();
        Preset::new(skin).paint(&mut list, &mut text, &data(active), BOUNDS, visual);
        list.finish()
    }

    fn fills(list: &DrawList) -> Vec<(Rect, Paint)> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Fill {
                    geom: Geom::Rect(rect),
                    paint,
                } => Some((*rect, *paint)),
                _ => None,
            })
            .collect()
    }

    fn clips(list: &DrawList) -> Vec<Rect> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Clip { region, .. } => Some(*region),
                _ => None,
            })
            .collect()
    }

    #[kithara::test]
    fn geometry_uses_the_selector_size_padding_gap_and_frames_from_the_skin() {
        let skin = builtin::skin();
        let painter = Preset::new(skin);
        let selector = painter.selector(BOUNDS);
        let list = draw(Some(0), IndexedVisual::default());
        let fills = fills(&list);
        let chip_width = (selector.w - skin.global_bar.chip_gap) / 2.0;

        assert_eq!(
            painter.declared(),
            Size::new(
                Length::Fixed(skin.global_bar.selector_width),
                Length::Fixed(skin.global_bar.height),
            )
        );
        assert_eq!(
            selector,
            Rect {
                h: BOUNDS.h - skin.global_bar.selector_padding_y * 2.0,
                w: BOUNDS.w - skin.global_bar.selector_padding_x * 2.0,
                x: BOUNDS.x + skin.global_bar.selector_padding_x,
                y: BOUNDS.y + skin.global_bar.selector_padding_y,
            }
        );
        assert_eq!(fills[0].0, BOUNDS);
        assert_eq!(fills[0].1, Paint::Solid(skin.palette.bg_panel));
        assert_eq!(fills[1].0, selector);
        assert_eq!(fills[1].1, Paint::Solid(skin.palette.line));
        assert_eq!(
            fills[2].0,
            Rect {
                w: chip_width,
                ..selector
            }
        );
        assert_eq!(
            fills[3].0,
            Rect {
                x: selector.x + chip_width + skin.global_bar.chip_gap,
                w: chip_width,
                ..selector
            }
        );
        assert_eq!(
            clips(&list),
            [
                Rect {
                    h: selector.h - skin.global_bar.chip_padding_y * 2.0,
                    w: chip_width - skin.global_bar.chip_padding_x * 2.0,
                    x: selector.x + skin.global_bar.chip_padding_x,
                    y: selector.y + skin.global_bar.chip_padding_y,
                },
                Rect {
                    h: selector.h - skin.global_bar.chip_padding_y * 2.0,
                    w: chip_width - skin.global_bar.chip_padding_x * 2.0,
                    x: selector.x
                        + chip_width
                        + skin.global_bar.chip_gap
                        + skin.global_bar.chip_padding_x,
                    y: selector.y + skin.global_bar.chip_padding_y,
                },
            ]
        );
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCmd::Stroke { geom: Geom::Rect(rect), color, pen }
                if pen.width == skin.global_bar.selector_frame.border_width
                    && *color == skin.rgba(skin.global_bar.selector_frame.border)
                    && rect.w == selector.w - skin.global_bar.selector_frame.border_width
                    && rect.h == selector.h - skin.global_bar.selector_frame.border_width
        )));
    }

    #[kithara::test]
    fn active_inactive_hovered_and_pressed_cells_are_distinct_skin_colours() {
        let skin = builtin::skin();
        let idle = fills(&draw(Some(0), IndexedVisual::default()));
        let hovered = fills(&draw(
            Some(0),
            IndexedVisual {
                hovered: Some(0),
                pressed_origin: None,
            },
        ));
        let pressed = fills(&draw(
            Some(0),
            IndexedVisual {
                hovered: Some(1),
                pressed_origin: Some(1),
            },
        ));

        assert_eq!(idle[2].1, Paint::Solid(skin.palette.accent));
        assert_eq!(idle[3].1, Paint::Solid(skin.palette.bg_panel));
        assert_eq!(hovered[2].1, Paint::Solid(skin.palette.accent_strong));
        assert_eq!(hovered[3].1, Paint::Solid(skin.palette.bg_panel));
        assert_eq!(pressed[2].1, Paint::Solid(skin.palette.accent));
        assert_eq!(pressed[3].1, Paint::Solid(skin.palette.accent_soft));

        let text = draw(Some(0), IndexedVisual::default())
            .commands()
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Clip { list, .. } => list.commands().first(),
                _ => None,
            })
            .filter_map(|command| match command {
                DrawCmd::Text {
                    content,
                    run,
                    color,
                    ..
                } => Some((content.clone(), run.clone(), *color)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let role = TextRoleSkin {
            color: ColorRole::TextDim,
            font: FontFamily::Mono,
            size: skin.global_bar.chip_text.size,
            spacing: 0.0,
            weight: skin.global_bar.chip_text.weight,
        };
        let mut shaper = TextContext::from(skin.text_resources());
        assert_eq!(text[0].0, "MICRO");
        assert_eq!(text[0].1, shaper.shape("MICRO", role, None));
        assert_eq!(text[0].2, skin.palette.bg);
        assert_eq!(text[1].0, "PLAYER");
        assert_eq!(text[1].1, shaper.shape("PLAYER", role, None));
        assert_eq!(text[1].2, skin.palette.text_dim);
    }
}
