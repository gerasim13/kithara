use num_traits::ToPrimitive;

use crate::{
    atoms::tree::Tree,
    draw::{DrawList, DrawListBuilder, Pt, Rect},
    engine::TextInputSnapshot,
    shaping::TextContext,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Drawn {
    pub(crate) hovered: Option<usize>,
    pub(crate) search: TextInputSnapshot,
    pub(crate) offset: f32,
}

impl Tree {
    pub(crate) fn commands(&self, text: &mut TextContext, bounds: Rect, drawn: &Drawn) -> DrawList {
        let search = self.search_height();
        let panel = Rect {
            h: (bounds.h - search).max(0.0),
            w: bounds.w,
            x: bounds.x,
            y: bounds.y + search,
        };
        let mut list = DrawListBuilder::default();
        list.fill_rect(panel, self.skin().rgba(self.skin().tree.panel_background));
        if let Some(query) = self.query() {
            crate::atoms::search::paint::paint(
                &mut list,
                text,
                bounds,
                query,
                self.skin(),
                &drawn.search,
            );
        }
        self.paint_rows(
            &mut list,
            text,
            self.rows_bounds(bounds),
            drawn.offset,
            drawn.hovered,
        );
        list.finish()
    }

    pub(crate) fn hovered_row(
        &self,
        point: Option<Pt>,
        bounds: Rect,
        offset: f32,
    ) -> Option<usize> {
        let viewport = self.rows_bounds(bounds);
        let point = point.filter(|point| viewport.contains(*point))?;
        if self.skin().tree.row_height <= 0.0 {
            return None;
        }
        let content_height = self.row_count().to_f32()? * self.skin().tree.row_height;
        let right_inset = if content_height > viewport.h {
            self.skin().tree.scrollbar_margin + self.skin().tree.scrollbar_width
        } else {
            0.0
        };
        if point.x >= viewport.x + (viewport.w - right_inset).max(0.0) {
            return None;
        }
        ((point.y - viewport.y + offset) / self.skin().tree.row_height)
            .floor()
            .to_usize()
            .filter(|index| *index < self.row_count())
    }

    pub(crate) fn rows_bounds(&self, bounds: Rect) -> Rect {
        let search = self.search_height();
        Rect {
            h: (bounds.h
                - search
                - self.skin().tree.panel_padding_top
                - self.skin().tree.panel_padding_bottom)
                .max(0.0),
            w: bounds.w,
            x: bounds.x,
            y: bounds.y + search + self.skin().tree.panel_padding_top,
        }
    }

    fn search_height(&self) -> f32 {
        if self.query().is_some() {
            self.skin().tree.search_height
        } else {
            0.0
        }
    }

    pub(crate) fn search_input_bounds(&self, bounds: Rect) -> Rect {
        crate::atoms::search::input_bounds(bounds, self.skin())
    }
}
