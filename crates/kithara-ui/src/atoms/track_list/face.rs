use num_traits::ToPrimitive;

use crate::{
    atoms::track_list::{
        ColumnLayout, TrackListRowData, column_label, track_list_body, track_list_content_height,
        track_list_content_width, track_list_dividers, track_list_overflows, track_list_row_pitch,
        track_list_row_rect, track_list_vertical_scrollbar_rect,
    },
    draw::{DrawList, DrawListBuilder, Pt, Rect, Rgba, Transform},
    interact::ScrollAxis,
    module::TrackColumn,
    render::Skin,
    skin::{ColorRole, FontFamily, FontSkin, FrameSkin, TextRoleSkin},
    text::TextContext,
};

#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct TrackList {
    #[field(get, vis = "pub(crate)")]
    columns: Vec<ColumnLayout>,
    #[field(get, vis = "pub(crate)")]
    rows: Vec<TrackListRowData>,
    #[field(get, vis = "pub(crate)")]
    skin: Skin,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Drawn {
    pub(crate) columns: Vec<ColumnLayout>,
    pub(crate) horizontal: f32,
    pub(crate) hovered: Option<usize>,
    pub(crate) pressed: Option<usize>,
    pub(crate) vertical: f32,
}

impl TrackList {
    pub(crate) fn new(
        rows: Vec<TrackListRowData>,
        columns: Vec<ColumnLayout>,
        skin: &Skin,
    ) -> Self {
        Self {
            columns,
            rows,
            skin: skin.clone(),
        }
    }

    pub(crate) fn commands(&self, text: &mut TextContext, bounds: Rect, drawn: &Drawn) -> DrawList {
        let overflowing = track_list_overflows(&drawn.columns, bounds.w);
        let horizontal = if overflowing { drawn.horizontal } else { 0.0 };
        let content_width = track_list_content_width(&drawn.columns, bounds.w);
        let mut content = DrawListBuilder::default();
        content.fill_rect(
            Rect {
                w: content_width,
                x: -horizontal,
                ..bounds
            },
            self.skin.palette.line_soft,
        );
        self.paint_header(&mut content, text, bounds, horizontal, &drawn.columns);
        self.paint_body(
            &mut content,
            text,
            bounds,
            (horizontal, drawn.vertical),
            (drawn.hovered, drawn.pressed),
            &drawn.columns,
        );
        paint_footer(self, &mut content, text, bounds, horizontal, &drawn.columns);
        paint_vertical_scrollbar(
            self,
            &mut content,
            bounds,
            horizontal,
            drawn.vertical,
            &drawn.columns,
        );
        if overflowing {
            paint_horizontal_scrollbar(self, &mut content, bounds, horizontal, &drawn.columns);
            let mut clipped = DrawListBuilder::default();
            clipped.clip(bounds, content.finish());
            clipped.finish()
        } else {
            content.finish()
        }
    }
    fn paint_header(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        bounds: Rect,
        horizontal: f32,
        columns: &[ColumnLayout],
    ) {
        let header = Rect {
            h: self.skin.track_list.header_height,
            w: track_list_content_width(columns, bounds.w),
            x: -horizontal,
            y: bounds.y,
        };
        list.fill_rect(header, self.skin.palette.bg_panel);
        for (column, cell) in column_cells(bounds, columns, horizontal) {
            let align = if column.column == TrackColumn::Index {
                TextAlign::Right
            } else {
                TextAlign::Left
            };
            paint_text(
                list,
                text,
                column_label(column.column, &self.skin.track_list),
                Rect {
                    h: header.h,
                    ..cell
                },
                (
                    self.skin.track_list.header_text,
                    FontFamily::Mono,
                    self.skin.palette.muted,
                    self.skin.track_list.cell_padding_x,
                    align,
                ),
            );
        }
        for divider in track_list_dividers(bounds, columns, horizontal, &self.skin) {
            list.fill_rect(
                divider.paint,
                self.skin.rgba(self.skin.track_list.divider_color),
            );
        }
    }

    fn paint_body(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        bounds: Rect,
        offsets: (f32, f32),
        interaction: (Option<usize>, Option<usize>),
        columns: &[ColumnLayout],
    ) {
        let (horizontal, vertical) = offsets;
        let body = track_list_body(bounds, &self.skin);
        let pitch = track_list_row_pitch(&self.skin);
        let visible = visible_rows(self.rows.len(), pitch, body.h, vertical);
        let mut rows = DrawListBuilder::default();
        for index in visible {
            let row_bounds =
                track_list_row_rect(bounds, columns, index, horizontal, vertical, &self.skin);
            self.paint_row(&mut rows, text, index, row_bounds, interaction, columns);
        }
        list.clip(body, rows.finish());
    }

    fn paint_row(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        index: usize,
        bounds: Rect,
        interaction: (Option<usize>, Option<usize>),
        columns: &[ColumnLayout],
    ) {
        let (hovered, pressed) = (interaction.0 == Some(index), interaction.1 == Some(index));
        let row = &self.rows[index];
        let frame = self.skin.track_list.row_frame;
        let fill = if pressed {
            self.skin.palette.accent_soft
        } else if row.selected {
            self.skin.palette.bg_select
        } else if hovered {
            self.skin.palette.bg_panel_2
        } else {
            self.skin.palette.bg_inset
        };
        list.fill_rounded_rect(bounds, frame.radius, fill);
        paint_frame(list, bounds, frame, &self.skin);
        for (column, cell) in column_cells(
            Rect {
                w: bounds.w,
                x: bounds.x,
                ..bounds
            },
            columns,
            0.0,
        ) {
            self.paint_cell(list, text, column.column, index, row, cell);
        }
        for divider in track_list_dividers(
            Rect {
                h: self.skin.track_list.header_height,
                w: bounds.w,
                x: bounds.x,
                y: 0.0,
            },
            columns,
            0.0,
            &self.skin,
        ) {
            list.fill_rect(
                Rect {
                    h: bounds.h,
                    y: bounds.y,
                    ..divider.paint
                },
                self.skin.rgba(self.skin.track_list.divider_color),
            );
        }
    }

    fn paint_cell(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        column: TrackColumn,
        index: usize,
        row: &TrackListRowData,
        bounds: Rect,
    ) {
        match column {
            TrackColumn::Index => paint_text(
                list,
                text,
                &format!("{:02}", index + 1),
                bounds,
                (
                    self.skin.track_list.index_text,
                    FontFamily::Mono,
                    self.skin.palette.muted,
                    self.skin.track_list.cell_padding_x,
                    TextAlign::Right,
                ),
            ),
            TrackColumn::Deck => paint_deck(self, list, text, row.deck.as_deref(), bounds),
            TrackColumn::Title => paint_text(
                list,
                text,
                value_or_dash(&row.title),
                bounds,
                (
                    self.skin.track_list.title_text,
                    FontFamily::Display,
                    self.skin.palette.text,
                    self.skin.track_list.cell_padding_x,
                    TextAlign::Left,
                ),
            ),
            TrackColumn::Artist => paint_text(
                list,
                text,
                optional_or_dash(row.artist.as_deref()),
                bounds,
                (
                    self.skin.track_list.artist_text,
                    FontFamily::Sans,
                    self.skin.palette.text_dim,
                    self.skin.track_list.cell_padding_x,
                    TextAlign::Left,
                ),
            ),
            TrackColumn::Bpm => paint_bpm(self, list, text, row.bpm.as_deref(), bounds),
            TrackColumn::Key => paint_text(
                list,
                text,
                optional_or_dash(row.key.as_deref()),
                bounds,
                (
                    self.skin.track_list.key_text,
                    FontFamily::Mono,
                    self.skin.palette.accent,
                    self.skin.track_list.cell_padding_x,
                    TextAlign::Left,
                ),
            ),
            TrackColumn::Time => paint_text(
                list,
                text,
                optional_or_dash(row.time.as_deref()),
                bounds,
                (
                    self.skin.track_list.time_text,
                    FontFamily::Mono,
                    self.skin.palette.text_dim,
                    self.skin.track_list.cell_padding_x,
                    TextAlign::Right,
                ),
            ),
            TrackColumn::Energy => paint_energy(self, list, text, row.energy, bounds),
            TrackColumn::Transition => {
                let transition = row
                    .transition
                    .as_deref()
                    .map_or_else(|| "\u{2014}".to_owned(), str::to_uppercase);
                paint_text(
                    list,
                    text,
                    &transition,
                    bounds,
                    (
                        self.skin.track_list.transition_text,
                        FontFamily::Mono,
                        self.skin.palette.muted,
                        self.skin.track_list.cell_padding_x,
                        TextAlign::Left,
                    ),
                );
            }
        }
    }
}

fn paint_deck(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    marks: Option<&str>,
    bounds: Rect,
) {
    let Some(marks) = marks else {
        return;
    };
    let chip = Rect {
        h: paint.skin.track_list.deck_chip_height,
        w: paint.skin.track_list.deck_chip_width,
        x: bounds.x + (bounds.w - paint.skin.track_list.deck_chip_width) / 2.0,
        y: bounds.y + (bounds.h - paint.skin.track_list.deck_chip_height) / 2.0,
    };
    let frame = paint.skin.track_list.deck_chip_frame;
    list.fill_rounded_rect(chip, frame.radius, paint.skin.palette.accent);
    paint_frame(list, chip, frame, &paint.skin);
    paint_text(
        list,
        text,
        marks,
        chip,
        (
            paint.skin.track_list.deck_text,
            FontFamily::Mono,
            paint.skin.palette.bg_deep,
            0.0,
            TextAlign::Center,
        ),
    );
}

fn paint_bpm(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    value: Option<&str>,
    bounds: Rect,
) {
    let content = optional_or_dash(value);
    let run = shape(
        text,
        content,
        paint.skin.track_list.bpm_text,
        FontFamily::Mono,
        None,
    );
    let badge = Rect {
        h: paint.skin.track_list.bpm_badge_height,
        w: run.width() + paint.skin.track_list.bpm_badge_padding_x * 2.0,
        x: bounds.x + paint.skin.track_list.cell_padding_x,
        y: bounds.y + (bounds.h - paint.skin.track_list.bpm_badge_height) / 2.0,
    };
    let frame = paint.skin.track_list.bpm_badge_frame;
    list.fill_rounded_rect(
        badge,
        frame.radius,
        paint.skin.rgba(paint.skin.track_list.bpm_badge_background),
    );
    paint_frame(list, badge, frame, &paint.skin);
    list.text(
        &run,
        content,
        Transform::translate(Pt {
            x: badge.x + paint.skin.track_list.bpm_badge_padding_x,
            y: badge.y + (badge.h - run.height()) / 2.0,
        }),
        paint.skin.palette.text,
    );
}

fn paint_energy(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    value: Option<u8>,
    bounds: Rect,
) {
    let value = value.map(|value| value.min(100));
    let ratio = value.map_or(0.0, |value| f32::from(value) / 100.0);
    let bar = Rect {
        h: paint.skin.track_list.energy_bar_height,
        w: paint.skin.track_list.energy_bar_width,
        x: bounds.x + paint.skin.track_list.cell_padding_x,
        y: bounds.y + (bounds.h - paint.skin.track_list.energy_bar_height) / 2.0,
    };
    list.fill_rect(
        bar,
        paint.skin.rgba(paint.skin.track_list.energy_bar_background),
    );
    list.fill_rect(
        Rect {
            w: bar.w * ratio,
            ..bar
        },
        paint.skin.palette.accent,
    );
    let label = value.map_or_else(|| "\u{2014}".to_owned(), |value| value.to_string());
    let label_x = bar.x + bar.w + paint.skin.track_list.energy_bar_gap;
    let label_bounds = Rect {
        h: bounds.h,
        w: (bounds.x + bounds.w - label_x - paint.skin.track_list.cell_padding_x).max(0.0),
        x: label_x,
        y: bounds.y,
    };
    paint_text(
        list,
        text,
        &label,
        label_bounds,
        (
            paint.skin.track_list.energy_text,
            FontFamily::Mono,
            paint.skin.palette.accent,
            0.0,
            TextAlign::Left,
        ),
    );
}

fn paint_footer(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    horizontal: f32,
    columns: &[ColumnLayout],
) {
    let footer = Rect {
        h: paint.skin.track_list.footer_height,
        w: track_list_content_width(columns, bounds.w),
        x: -horizontal,
        y: bounds.y + bounds.h - paint.skin.track_list.footer_height,
    };
    list.fill_rect(footer, paint.skin.palette.bg_footer);
    let label = format!(
        "{} {}",
        paint.rows.len(),
        paint.skin.track_list.labels.footer_tracks
    );
    paint_text(
        list,
        text,
        &label,
        footer,
        (
            paint.skin.track_list.footer_text,
            FontFamily::Mono,
            paint.skin.palette.muted,
            paint.skin.track_list.footer_padding_x,
            TextAlign::Left,
        ),
    );
}

fn paint_vertical_scrollbar(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    bounds: Rect,
    horizontal: f32,
    offset: f32,
    columns: &[ColumnLayout],
) {
    let body = track_list_body(bounds, &paint.skin);
    let content = track_list_content_height(paint.rows.len(), &paint.skin);
    let Some(rail) = track_list_vertical_scrollbar_rect(
        bounds,
        columns,
        paint.rows.len(),
        horizontal,
        &paint.skin,
    ) else {
        return;
    };
    paint_scrollbar(
        list,
        rail,
        content,
        body.h,
        offset,
        ScrollAxis::Vertical,
        &paint.skin,
    );
}

fn paint_horizontal_scrollbar(
    paint: &TrackList,
    list: &mut DrawListBuilder,
    bounds: Rect,
    offset: f32,
    columns: &[ColumnLayout],
) {
    paint_scrollbar(
        list,
        Rect {
            h: paint.skin.track_list.scrollbar_width,
            w: bounds.w,
            x: bounds.x,
            y: bounds.y + bounds.h
                - paint.skin.track_list.scrollbar_margin
                - paint.skin.track_list.scrollbar_width,
        },
        track_list_content_width(columns, bounds.w),
        bounds.w,
        offset,
        ScrollAxis::Horizontal,
        &paint.skin,
    );
}

#[derive(Clone, Copy)]
enum TextAlign {
    Left,
    Center,
    Right,
}

fn column_cells(
    bounds: Rect,
    columns: &[ColumnLayout],
    horizontal: f32,
) -> impl Iterator<Item = (ColumnLayout, Rect)> + '_ {
    let minimum = columns.iter().map(|column| column.width).sum::<f32>();
    let title_extra = (bounds.w - minimum).max(0.0);
    let mut x = bounds.x - horizontal;
    columns.iter().copied().map(move |column| {
        let width = column.width
            + if column.column == TrackColumn::Title {
                title_extra
            } else {
                0.0
            };
        let rect = Rect {
            h: bounds.h,
            w: width,
            x,
            y: bounds.y,
        };
        x += width;
        (column, rect)
    })
}

fn visible_rows(
    row_count: usize,
    pitch: f32,
    viewport: f32,
    offset: f32,
) -> std::ops::Range<usize> {
    if pitch <= 0.0 || viewport <= 0.0 {
        return 0..0;
    }
    let start = (offset.max(0.0) / pitch)
        .floor()
        .to_usize()
        .map_or(row_count, |index| index.min(row_count));
    let end = ((offset.max(0.0) + viewport) / pitch)
        .ceil()
        .to_usize()
        .map_or(row_count, |index| index.min(row_count));
    start..end.max(start)
}

fn paint_text(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    content: &str,
    bounds: Rect,
    paint: (FontSkin, FontFamily, Rgba, f32, TextAlign),
) {
    let (font, family, color, padding_x, align) = paint;
    let available = (bounds.w - padding_x * 2.0).max(0.0);
    let run = shape(text, content, font, family, Some(available));
    let x = match align {
        TextAlign::Left => bounds.x + padding_x,
        TextAlign::Center => bounds.x + (bounds.w - run.width()) / 2.0,
        TextAlign::Right => bounds.x + bounds.w - padding_x - run.width(),
    };
    list.text(
        &run,
        content,
        Transform::translate(Pt {
            x,
            y: bounds.y + (bounds.h - run.height()) / 2.0,
        }),
        color,
    );
}

fn shape(
    text: &mut TextContext,
    content: &str,
    font: FontSkin,
    family: FontFamily,
    max_width: Option<f32>,
) -> crate::text::GlyphRun {
    text.shape(
        content,
        TextRoleSkin {
            color: ColorRole::Text,
            font: family,
            size: font.size,
            spacing: 0.0,
            weight: font.weight,
        },
        max_width,
    )
}

fn paint_frame(list: &mut DrawListBuilder, bounds: Rect, frame: FrameSkin, skin: &Skin) {
    if frame.border_width <= 0.0 {
        return;
    }
    let inset = frame.border_width / 2.0;
    list.stroke_rounded_rect(
        Rect {
            h: (bounds.h - frame.border_width).max(0.0),
            w: (bounds.w - frame.border_width).max(0.0),
            x: bounds.x + inset,
            y: bounds.y + inset,
        },
        frame.radius,
        skin.rgba(frame.border),
        frame.border_width,
    );
}

fn paint_scrollbar(
    list: &mut DrawListBuilder,
    rail: Rect,
    content_extent: f32,
    viewport_extent: f32,
    offset: f32,
    axis: ScrollAxis,
    skin: &Skin,
) {
    let maximum = (content_extent - viewport_extent).max(0.0);
    if viewport_extent <= 0.0 || maximum <= 0.0 {
        return;
    }
    let track_extent = match axis {
        ScrollAxis::Horizontal => rail.w,
        ScrollAxis::Vertical => rail.h,
    };
    let thumb_extent = (track_extent * viewport_extent / content_extent)
        .max(skin.track_list.scrollbar_width)
        .min(track_extent);
    let thumb_offset = offset.clamp(0.0, maximum) / maximum * (track_extent - thumb_extent);
    let thumb = match axis {
        ScrollAxis::Horizontal => Rect {
            w: thumb_extent,
            x: rail.x + thumb_offset,
            ..rail
        },
        ScrollAxis::Vertical => Rect {
            h: thumb_extent,
            y: rail.y + thumb_offset,
            ..rail
        },
    };
    list.fill_rect(rail, skin.rgba(skin.track_list.scrollbar_background));
    list.fill_rect(thumb, skin.rgba(skin.track_list.scroller_color));
}

fn value_or_dash(value: &str) -> &str {
    if value.is_empty() { "\u{2014}" } else { value }
}

fn optional_or_dash(value: Option<&str>) -> &str {
    value.map_or("\u{2014}", value_or_dash)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::track_list::track_list_body,
        builtin,
        draw::{DrawCmd, Geom},
    };

    #[kithara::test]
    fn scrolling_changes_the_table_picture() {
        let (picture, mut text, bounds, drawn) = fixture();
        let unscrolled = picture.commands(&mut text, bounds, &drawn);
        let scrolled = picture.commands(
            &mut text,
            bounds,
            &Drawn {
                vertical: picture.skin.track_list.row_height,
                ..drawn
            },
        );

        assert_ne!(scrolled, unscrolled);
    }

    #[kithara::test]
    fn hovering_a_row_changes_its_picture() {
        let (picture, mut text, bounds, drawn) = fixture();
        let idle = picture.commands(&mut text, bounds, &drawn);
        let hovered = picture.commands(
            &mut text,
            bounds,
            &Drawn {
                hovered: Some(0),
                ..drawn
            },
        );

        assert_ne!(hovered, idle);
    }

    #[kithara::test]
    fn a_partial_bottom_row_stays_inside_the_body_clip() {
        let (picture, mut text, mut bounds, drawn) = fixture();
        bounds.h = picture.skin.track_list.header_height
            + picture.skin.track_list.footer_height
            + picture.skin.track_list.grid_gap * 2.0
            + picture.skin.track_list.row_height / 2.0;
        let body = track_list_body(bounds, &picture.skin);
        let commands = picture.commands(&mut text, bounds, &drawn);
        let clipped = commands
            .commands()
            .iter()
            .find_map(|command| match command {
                DrawCmd::Clip { region, list } if *region == body => Some(list),
                _ => None,
            })
            .unwrap_or_else(|| panic!("TrackList rows must be scoped to the body clip"));
        let row_bottom = clipped.commands().iter().find_map(|command| match command {
            DrawCmd::Fill {
                geom: Geom::Rect(rect) | Geom::RoundedRect { rect, .. },
                ..
            } => Some(rect.y + rect.h),
            _ => None,
        });

        assert_eq!(
            row_bottom,
            Some(body.y + picture.skin.track_list.row_height)
        );
        assert!(row_bottom.is_some_and(|bottom| bottom > body.y + body.h));
    }

    fn fixture() -> (TrackList, TextContext, Rect, Drawn) {
        let skin = builtin::skin();
        let columns = vec![ColumnLayout {
            column: TrackColumn::Title,
            width: 180.0,
        }];
        let rows = (0..4)
            .map(|index| TrackListRowData {
                artist: Some("Artist".to_owned()),
                bpm: Some("128".to_owned()),
                deck: None,
                energy: Some(7),
                key: Some("Am".to_owned()),
                time: Some("03:24".to_owned()),
                transition: None,
                title: format!("Track {index}"),
                selected: false,
            })
            .collect();
        let picture = TrackList::new(rows, columns.clone(), skin);
        (
            picture,
            TextContext::from(skin.text_resources()),
            Rect {
                h: 160.0,
                w: 180.0,
                x: 0.0,
                y: 0.0,
            },
            Drawn {
                columns,
                horizontal: 0.0,
                hovered: None,
                pressed: None,
                vertical: 0.0,
            },
        )
    }
}
