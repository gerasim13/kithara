use num_traits::ToPrimitive;

use super::{
    ColumnLayout, layout::intersect, track_list_body, track_list_content_width,
    track_list_row_pitch, track_list_vertical_scrollbar_rect,
};
use crate::{
    atoms::table::TableCell,
    draw::{Pt, Rect},
    module::TrackColumn,
    render::{Skin, TrackRow},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrackListRowData {
    pub(crate) artist: Option<String>,
    pub(crate) bpm: Option<String>,
    pub(crate) deck: Option<String>,
    pub(crate) energy: Option<u8>,
    pub(crate) key: Option<String>,
    pub(crate) time: Option<String>,
    pub(crate) transition: Option<String>,
    pub(crate) title: String,
    pub(crate) selected: bool,
}

impl From<&TrackRow<'_>> for TrackListRowData {
    fn from(track: &TrackRow<'_>) -> Self {
        Self {
            artist: track.artist.map(str::to_owned),
            bpm: track.bpm.map(str::to_owned),
            deck: track.deck.map(str::to_owned),
            energy: track.energy,
            key: track.key.map(str::to_owned),
            selected: track.selected,
            time: track.time.map(str::to_owned),
            title: track.title.to_owned(),
            transition: track.transition.map(str::to_owned),
        }
    }
}

impl TrackListRowData {
    pub(super) fn cell(&self, column: TrackColumn) -> TableCell {
        match column {
            TrackColumn::Index => TableCell::Empty,
            TrackColumn::Deck => text_cell(&self.deck),
            TrackColumn::Title => TableCell::Text(self.title.clone()),
            TrackColumn::Artist => text_cell(&self.artist),
            TrackColumn::Bpm => text_cell(&self.bpm),
            TrackColumn::Key => text_cell(&self.key),
            TrackColumn::Time => text_cell(&self.time),
            TrackColumn::Energy => self.energy.map_or(TableCell::Empty, TableCell::Number),
            TrackColumn::Transition => text_cell(&self.transition),
        }
    }
}

fn text_cell(value: &Option<String>) -> TableCell {
    value.clone().map_or(TableCell::Empty, TableCell::Text)
}

pub(crate) fn track_list_row_rect(
    bounds: Rect,
    columns: &[ColumnLayout],
    index: usize,
    horizontal_offset: f32,
    vertical_offset: f32,
    skin: &Skin,
) -> Rect {
    let body = track_list_body(bounds, skin);
    let y = index.to_f32().map_or(f32::MAX, |index| {
        index.mul_add(track_list_row_pitch(skin), body.y) - vertical_offset
    });
    Rect {
        h: skin.track_list.row_height,
        w: track_list_content_width(columns, bounds.w),
        x: bounds.x - horizontal_offset,
        y,
    }
}

pub(crate) fn track_list_visible_row_rect(
    bounds: Rect,
    columns: &[ColumnLayout],
    row_count: usize,
    index: usize,
    horizontal_offset: f32,
    vertical_offset: f32,
    skin: &Skin,
) -> Option<Rect> {
    let row = track_list_row_rect(
        bounds,
        columns,
        index,
        horizontal_offset,
        vertical_offset,
        skin,
    );
    let mut visible = intersect(row, track_list_body(bounds, skin))?;
    if let Some(scrollbar) =
        track_list_vertical_scrollbar_rect(bounds, columns, row_count, horizontal_offset, skin)
    {
        visible.w = (scrollbar.x - visible.x).max(0.0);
    }
    (visible.w > 0.0).then_some(visible)
}

pub(crate) fn track_list_row_at(
    point: Option<Pt>,
    bounds: Rect,
    columns: &[ColumnLayout],
    row_count: usize,
    horizontal_offset: f32,
    vertical_offset: f32,
    skin: &Skin,
) -> Option<usize> {
    let point = point?;
    let body = track_list_body(bounds, skin);
    let pitch = track_list_row_pitch(skin);
    if !body.contains(point) || pitch <= 0.0 {
        return None;
    }
    let y = point.y - body.y + vertical_offset;
    let index = (y / pitch).floor().to_usize()?;
    if index >= row_count {
        return None;
    }
    let row = track_list_visible_row_rect(
        bounds,
        columns,
        row_count,
        index,
        horizontal_offset,
        vertical_offset,
        skin,
    )?;
    row.contains(point).then_some(index)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::track_list::{column_layouts, minimum_table_width},
        module::TrackColumn,
        render::{ReadValue, Reads},
    };

    struct ColumnReads(Option<bool>);

    impl Reads for ColumnReads {
        fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
            (endpoint == "columns.title")
                .then_some(self.0)
                .flatten()
                .map(ReadValue::Bool)
        }
    }

    #[kithara::test]
    fn row_geometry_keeps_grid_gaps_outside_row_hits() {
        let skin = crate::builtin::skin();
        let columns = column_layouts(&[TrackColumn::Title], &ColumnReads(None), None, skin);
        let bounds = Rect {
            h: 160.0,
            w: 400.0,
            x: 0.0,
            y: 0.0,
        };
        let first = track_list_row_rect(bounds, &columns, 0, 0.0, 0.0, skin);
        let second = track_list_row_rect(bounds, &columns, 1, 0.0, 0.0, skin);

        assert_eq!(second.y - first.y, track_list_row_pitch(skin));
        assert_eq!(second.y - (first.y + first.h), skin.track_list.grid_gap);
    }

    #[kithara::test]
    fn visible_row_hits_are_clipped_to_the_body() {
        let skin = crate::builtin::skin();
        let columns = column_layouts(&[TrackColumn::Title], &ColumnReads(None), None, skin);
        let bounds = Rect {
            h: 160.0,
            w: 400.0,
            x: 0.0,
            y: 0.0,
        };
        let clipped = track_list_visible_row_rect(
            bounds,
            &columns,
            3,
            0,
            0.0,
            skin.track_list.row_height / 2.0,
            skin,
        )
        .unwrap_or_else(|| panic!("the partially visible first row must retain a hit rect"));

        assert_eq!(clipped.y, track_list_body(bounds, skin).y);
        assert_eq!(clipped.h, skin.track_list.row_height / 2.0);
    }

    #[kithara::test]
    fn row_hits_yield_to_the_visible_scrollbar_lane_at_each_horizontal_edge() {
        let skin = crate::builtin::skin();
        let columns = column_layouts(
            &[
                TrackColumn::Title,
                TrackColumn::Artist,
                TrackColumn::Transition,
            ],
            &ColumnReads(None),
            None,
            skin,
        );
        let bounds = Rect {
            h: 160.0,
            w: 400.0,
            x: 0.0,
            y: 0.0,
        };
        let row_count = 10;
        let maximum = minimum_table_width(&columns) - bounds.w;
        let row = |offset| {
            track_list_visible_row_rect(bounds, &columns, row_count, 0, offset, 0.0, skin)
                .unwrap_or_else(|| panic!("the first row must be visible"))
        };

        assert_eq!(
            track_list_vertical_scrollbar_rect(bounds, &columns, row_count, 0.0, skin),
            None
        );
        assert_eq!(row(0.0).w, bounds.w);

        let partial = maximum - skin.track_list.scrollbar_margin;
        let partial_scrollbar =
            track_list_vertical_scrollbar_rect(bounds, &columns, row_count, partial, skin)
                .unwrap_or_else(|| {
                    panic!("the rail must enter the viewport before maximum scroll")
                });
        assert_eq!(row(partial).x + row(partial).w, partial_scrollbar.x);

        let scrollbar =
            track_list_vertical_scrollbar_rect(bounds, &columns, row_count, maximum, skin)
                .unwrap_or_else(|| panic!("the rail must be visible at maximum horizontal scroll"));
        let visible = row(maximum);
        assert_eq!(visible.x + visible.w, scrollbar.x);
        let y = visible.y + visible.h / 2.0;
        assert_eq!(
            track_list_row_at(
                Some(Pt {
                    x: scrollbar.x - 0.5,
                    y,
                }),
                bounds,
                &columns,
                row_count,
                maximum,
                0.0,
                skin,
            ),
            Some(0)
        );
        assert_eq!(
            track_list_row_at(
                Some(Pt { x: scrollbar.x, y }),
                bounds,
                &columns,
                row_count,
                maximum,
                0.0,
                skin,
            ),
            None
        );
    }
}
