use num_traits::ToPrimitive;

use crate::{
    module::TrackColumn,
    render::{ReadValue, Reads, Skin},
    skin::TrackListSkin,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColumnLayout {
    pub(crate) column: TrackColumn,
    pub(crate) width: f32,
}

pub(crate) fn column_resizable(columns: &[ColumnLayout], index: usize) -> bool {
    columns
        .get(index)
        .is_some_and(|column| column.column != TrackColumn::Title && index + 1 < columns.len())
}

fn column_visible(reads: &dyn Reads, state: Option<(&str, &str)>, column: TrackColumn) -> bool {
    let Some((prefix, scope)) = state else {
        return true;
    };
    let endpoint = format!("{prefix}.{}{scope}", column.endpoint_name());
    !matches!(reads.get(&endpoint), Some(ReadValue::Bool(false)))
}

pub(crate) fn column_layouts(
    columns: &[TrackColumn],
    reads: &dyn Reads,
    state: Option<(&str, &str)>,
    skin: &Skin,
) -> Vec<ColumnLayout> {
    columns
        .iter()
        .copied()
        .filter(|column| column_visible(reads, state, *column))
        .map(|column| ColumnLayout {
            column,
            width: effective_column_width(reads, state, column, skin),
        })
        .collect()
}

fn default_column_width(column: TrackColumn, skin: &Skin) -> f32 {
    match column {
        TrackColumn::Index => skin.track_list.index_width,
        TrackColumn::Deck => skin.track_list.deck_width,
        TrackColumn::Title => skin.track_list.title_min_width,
        TrackColumn::Artist => skin.track_list.artist_width,
        TrackColumn::Bpm => skin.track_list.bpm_width,
        TrackColumn::Key => skin.track_list.key_width,
        TrackColumn::Time => skin.track_list.time_width,
        TrackColumn::Energy => skin.track_list.energy_width,
        TrackColumn::Transition => skin.track_list.transition_width,
    }
}

fn effective_column_width(
    reads: &dyn Reads,
    state: Option<(&str, &str)>,
    column: TrackColumn,
    skin: &Skin,
) -> f32 {
    let default = default_column_width(column, skin);
    let Some((prefix, scope)) = state else {
        return default;
    };
    let endpoint = format!("{prefix}.width.{}{scope}", column.endpoint_name());
    let Some(ReadValue::Scalar(width)) = reads.get(&endpoint) else {
        return default;
    };
    let Some(width) = width.to_f32().filter(|width| width.is_finite()) else {
        return default;
    };
    let minimum = if column == TrackColumn::Title {
        skin.track_list.title_min_width
    } else {
        skin.track_list.min_column_width
    };
    width.max(minimum)
}

pub(crate) fn minimum_table_width(columns: &[ColumnLayout]) -> f32 {
    columns.iter().map(|column| column.width).sum()
}

pub(crate) fn column_label(column: TrackColumn, metrics: &TrackListSkin) -> &str {
    let labels = &metrics.labels;
    match column {
        TrackColumn::Index => &labels.index,
        TrackColumn::Deck => &labels.deck,
        TrackColumn::Title => &labels.title,
        TrackColumn::Artist => &labels.artist,
        TrackColumn::Bpm => &labels.bpm,
        TrackColumn::Key => &labels.key,
        TrackColumn::Time => &labels.time,
        TrackColumn::Energy => &labels.energy,
        TrackColumn::Transition => &labels.transition,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    struct ColumnReads(Option<bool>);

    impl Reads for ColumnReads {
        fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
            (endpoint == "columns.title")
                .then_some(self.0)
                .flatten()
                .map(ReadValue::Bool)
        }
    }

    struct WidthReads;

    impl Reads for WidthReads {
        fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
            match endpoint {
                "columns.index" => Some(ReadValue::Bool(false)),
                "columns.width.artist" => Some(ReadValue::Scalar(240.0)),
                _ => None,
            }
        }
    }

    #[kithara::test]
    fn absent_column_endpoint_is_visible() {
        assert!(column_visible(
            &ColumnReads(None),
            Some(("columns", "")),
            TrackColumn::Title
        ));
    }

    #[kithara::test]
    fn false_column_endpoint_is_hidden() {
        assert!(!column_visible(
            &ColumnReads(Some(false)),
            Some(("columns", "")),
            TrackColumn::Title
        ));
    }

    #[kithara::test]
    fn total_width_uses_host_override_and_title_minimum() {
        let skin = crate::builtin::skin();
        let columns = column_layouts(
            &[TrackColumn::Index, TrackColumn::Title, TrackColumn::Artist],
            &WidthReads,
            Some(("columns", "")),
            skin,
        );

        assert_eq!(columns.len(), 2);
        assert_eq!(columns[1].width, 240.0);
        assert_eq!(
            minimum_table_width(&columns),
            skin.track_list.title_min_width + 240.0
        );
    }
}
