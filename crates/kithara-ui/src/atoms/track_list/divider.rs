use super::{ColumnLayout, layout::intersect};
use crate::{draw::Rect, module::TrackColumn, render::Skin};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColumnDividerLayout {
    pub(crate) column: TrackColumn,
    pub(crate) hit: Rect,
    pub(crate) paint: Rect,
    pub(crate) value: f32,
}

pub(crate) fn track_list_dividers(
    bounds: Rect,
    columns: &[ColumnLayout],
    horizontal_offset: f32,
    skin: &Skin,
) -> Vec<ColumnDividerLayout> {
    let flexible_title = !super::track_list_overflows(columns, bounds.w);
    let extra = (bounds.w - super::minimum_table_width(columns)).max(0.0);
    let mut edge = bounds.x - horizontal_offset;
    let mut dividers = Vec::new();
    for (index, column) in columns.iter().copied().enumerate() {
        let width = if flexible_title && column.column == TrackColumn::Title {
            column.width + extra
        } else {
            column.width
        };
        edge += width;
        if !super::column_resizable(columns, index) {
            continue;
        }
        dividers.push(ColumnDividerLayout {
            column: column.column,
            hit: Rect {
                h: skin.track_list.header_height,
                w: skin.track_list.divider_hit_width,
                x: edge - skin.track_list.divider_hit_width / 2.0,
                y: bounds.y,
            },
            paint: Rect {
                h: skin.track_list.header_height,
                w: skin.track_list.divider_width,
                x: edge - skin.track_list.divider_width / 2.0,
                y: bounds.y,
            },
            value: column.width,
        });
    }
    dividers
}

pub(crate) fn track_list_visible_divider_hit(bounds: Rect, hit: Rect) -> Option<Rect> {
    intersect(hit, bounds)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::track_list::column_layouts,
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
    fn divider_hit_rect_is_wider_than_the_centered_paint_rect() {
        let skin = crate::builtin::skin();
        let columns = column_layouts(
            &[TrackColumn::Index, TrackColumn::Title, TrackColumn::Artist],
            &ColumnReads(None),
            None,
            skin,
        );
        let divider = track_list_dividers(
            Rect {
                h: 160.0,
                w: 800.0,
                x: 0.0,
                y: 0.0,
            },
            &columns,
            0.0,
            skin,
        )[0];

        assert_eq!(divider.hit.w, skin.track_list.divider_hit_width);
        assert_eq!(divider.paint.w, skin.track_list.divider_width);
        assert_eq!(divider.hit.w, 7.0);
        assert_eq!(divider.paint.w, 1.0);
        assert!(divider.hit.w > divider.paint.w);
        assert_eq!(
            divider.hit.x + divider.hit.w / 2.0,
            divider.paint.x + divider.paint.w / 2.0
        );
    }

    #[kithara::test]
    fn divider_hit_bands_are_clipped_at_both_viewport_edges() {
        let bounds = Rect {
            h: 100.0,
            w: 100.0,
            x: 0.0,
            y: 0.0,
        };
        let hit = |x, w| Rect {
            h: 22.0,
            w,
            x,
            y: 0.0,
        };

        assert_eq!(track_list_visible_divider_hit(bounds, hit(-8.0, 4.0)), None);
        assert_eq!(
            track_list_visible_divider_hit(bounds, hit(-2.0, 7.0)),
            Some(hit(0.0, 5.0))
        );
        assert_eq!(
            track_list_visible_divider_hit(bounds, hit(98.0, 7.0)),
            Some(hit(98.0, 2.0))
        );
        assert_eq!(
            track_list_visible_divider_hit(bounds, hit(101.0, 7.0)),
            None
        );
    }
}
