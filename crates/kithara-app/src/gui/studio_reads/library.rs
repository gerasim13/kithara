use kithara_ui::render::{Node, ReadValue, Scope, TableCell, TableRow, TableValue};

use super::value::Value;
use crate::{catalog::Catalog, gui::studio_ui::cache::CatalogRowMarks};

pub(super) struct LibraryNode<'a> {
    rows: Vec<TableRow<'a>>,
}

impl<'a> LibraryNode<'a> {
    pub(super) fn new(
        catalog: &'a Catalog,
        marks: &'a CatalogRowMarks,
        selected: Option<usize>,
    ) -> Self {
        let rows = catalog
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let deck = marks
                    .get(index)
                    .map(String::as_str)
                    .filter(|marks| !marks.is_empty())
                    .map_or_else(
                        || TableCell::empty("deck"),
                        |marks| TableCell::text("deck", marks),
                    );
                TableRow::new(
                    vec![
                        TableCell::empty("index"),
                        deck,
                        TableCell::text("title", &entry.name),
                        TableCell::text("artist", entry.url.as_str()),
                        TableCell::empty("bpm"),
                        TableCell::empty("key"),
                        TableCell::empty("time"),
                        TableCell::empty("energy"),
                        TableCell::empty("transition"),
                    ],
                    selected == Some(index),
                )
            })
            .collect();
        Self { rows }
    }

    pub(super) fn title(&self, row: usize) -> Option<&'a str> {
        self.rows
            .get(row)?
            .cells()
            .iter()
            .find(|cell| cell.id() == "title")
            .and_then(|cell| match cell.value() {
                TableValue::Text(value) => Some(value),
                _ => None,
            })
    }
}

impl<'a, 'b: 'a> Node<'a> for &'a LibraryNode<'b> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let rows: &'a [TableRow<'a>] = &self.rows;
        let value = match segment {
            "tracks" => ReadValue::Table(rows),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}
