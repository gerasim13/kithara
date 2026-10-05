use std::any::Any;

use iced::{
    Element, Event, Rectangle, Renderer, Theme,
    advanced::widget::{Id, Operation},
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Geometry},
};
use kithara_platform::time::Instant;

use super::{
    super::{
        Carried, ControlAction, InputOwner, Published, control_event, controls::RetainedCanvas,
        drag, index, scalar,
    },
    paint::{TableConfig, TablePaint, TableState, hovered_row, local_rect},
};
use crate::{
    atoms::table::{face::TableFace, table_body, table_dividers, table_visible_row_rect},
    draw::{Pt, Rect},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome, PointerPhase, iced as iced_interact,
        recognizers::{ItemDrag, Scalar, Track, click},
    },
};

pub(crate) fn table<'skin>(
    path: &str,
    face: TableFace,
    owner: InputOwner,
) -> Element<'skin, Published> {
    let paint = TablePaint::new(path, face);
    let config = paint.config();
    match owner {
        InputOwner::Leaf => RetainedCanvas::new(
            TableProgram {
                paint,
                config: config.clone(),
            },
            path,
            config,
        )
        .view(),
        InputOwner::Engine => RetainedCanvas::new(paint, path, config).view(),
    }
}

pub(crate) fn carried(state: &dyn Any, path: &str, index: usize) -> Option<Carried> {
    state.downcast_ref::<TableState>()?.carried(path, index)
}

pub(crate) fn sync_table_scroll(
    path: &str,
    horizontal: f32,
    pressed: Option<usize>,
    vertical: f32,
) -> impl Operation + '_ {
    struct Sync<'a> {
        horizontal: f32,
        path: &'a str,
        pressed: Option<usize>,
        vertical: f32,
    }

    impl Operation for Sync<'_> {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Any) {
            if let Some(state) = state.downcast_mut::<TableState>() {
                state.sync(self.path, self.horizontal, self.pressed, self.vertical);
            }
        }
    }

    Sync {
        horizontal,
        path,
        pressed,
        vertical,
    }
}

pub(super) struct TableProgram {
    pub(super) config: TableConfig,
    pub(super) paint: TablePaint,
}

impl canvas::Program<Published> for TableProgram {
    type State = TableState;

    fn draw(
        &self,
        state: &TableState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.geometry(state, renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &TableState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        let point = cursor.position_in(bounds).map(Into::into);
        let bounds = local_rect(bounds);
        let dividers = table_dividers(
            bounds,
            self.paint.face.columns(),
            state.horizontal.offset(),
            self.paint.face.metrics(),
        );
        for divider in &dividers {
            let Some((_, drag_state)) = state
                .dividers
                .iter()
                .find(|(column, _)| *column == divider.column)
            else {
                continue;
            };
            let drag = divider_drag(divider.track);
            let hit = Hit::new(point, divider.hit);
            let cursor = drag.cursor(drag_state, &hit);
            if cursor != CursorShape::None {
                return cursor.into();
            }
        }
        let row_cursor = state.row_drag.cursor();
        if row_cursor != CursorShape::None {
            return row_cursor.into();
        }
        if hovered_row(
            point,
            bounds,
            self.paint.face.rows().len(),
            state.horizontal.offset(),
            state.vertical.offset(),
            &self.paint.face,
        )
        .is_some()
        {
            Interaction::Pointer
        } else {
            Interaction::None
        }
    }

    fn update(
        &self,
        state: &mut TableState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Published>> {
        let input = iced_interact::input(event)?;
        let drag_point = cursor.position().map(Into::into);
        let point = cursor.position_in(bounds).map(Into::into);
        let viewport = bounds.size();
        let origin = Pt {
            x: bounds.x,
            y: bounds.y,
        };
        let bounds = local_rect(bounds);
        state.reconcile(&self.paint.path, &self.config);
        state.set_viewport(viewport, &self.config);

        if let Some(action) = self.divider_input(state, input, bounds, drag_point, origin) {
            return Some(action);
        }
        if let Some(action) = self.cell_action_input(state, input, bounds, point) {
            state.row_drag = ItemDrag::default();
            state.drag_index = None;
            state.pressed_index = None;
            return Some(action);
        }
        if let Some(action) = self.row_drag_input(state, input, bounds, point) {
            return Some(action);
        }

        let body = table_body(bounds, self.paint.face.metrics());
        let vertical_hit = Hit::new(point, body);
        let before = state.vertical.offset();
        let outcome = state.vertical.handle(input, &vertical_hit);
        let after = state.vertical.offset();
        if outcome.is_captured() || outcome.value().is_some() {
            return scroll_action(&self.paint.path, outcome, before, after);
        }

        let horizontal_hit = Hit::new(point, bounds);
        let before = state.horizontal.offset();
        let outcome = state.horizontal.handle(input, &horizontal_hit);
        let after = state.horizontal.offset();
        scroll_action(
            &format!("{}/scroll-x", self.paint.path),
            outcome,
            before,
            after,
        )
    }
}

impl TableProgram {
    fn cell_action_input(
        &self,
        state: &TableState,
        input: Input<'_>,
        bounds: Rect,
        point: Option<Pt>,
    ) -> Option<Action<Published>> {
        let face = &self.paint.face;
        let offsets = (state.horizontal.offset(), state.vertical.offset());
        let (_, column, _, action) = face
            .actions_under(point, bounds, offsets, face.columns())
            .find(|(_, _, cell, _)| {
                click::on_input(input, &Hit::new(point, *cell)).is_captured()
            })?;
        let path = format!(
            "{}/{}",
            self.paint.path,
            face.columns()[column].column.action_slot()
        );
        Some(
            Action::publish(control_event(&path, ControlAction::Text(action.to_owned())))
                .and_capture(),
        )
    }

    fn divider_input(
        &self,
        state: &mut TableState,
        input: Input<'_>,
        bounds: Rect,
        point: Option<Pt>,
        origin: Pt,
    ) -> Option<Action<Published>> {
        let dividers = table_dividers(
            bounds,
            self.paint.face.columns(),
            state.horizontal.offset(),
            self.paint.face.metrics(),
        );
        for divider in &dividers {
            let Some((_, drag_state)) = state
                .dividers
                .iter_mut()
                .find(|(column, _)| *column == divider.column)
            else {
                continue;
            };
            let drag = divider_drag(divider.track);
            let hit = Rect {
                x: divider.hit.x + origin.x,
                y: divider.hit.y + origin.y,
                ..divider.hit
            };
            let outcome = drag.on_input(drag_state, input, &Hit::new(point, hit), Instant::now());
            if outcome.is_captured() || outcome.value().is_some() {
                let path = format!("{}/width/{}", self.paint.path, divider.column.id());
                return scalar(&path, outcome.map(f64::from));
            }
        }
        None
    }

    fn row_drag_input(
        &self,
        state: &mut TableState,
        input: Input<'_>,
        bounds: Rect,
        point: Option<Pt>,
    ) -> Option<Action<Published>> {
        if matches!(
            input,
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Down
        ) {
            state.row_drag = ItemDrag::default();
            state.drag_index = hovered_row(
                point,
                bounds,
                self.paint.face.rows().len(),
                state.horizontal.offset(),
                state.vertical.offset(),
                &self.paint.face,
            );
            state.pressed_index = state.drag_index;
        }
        let row_index = state.drag_index?;
        let visible = table_visible_row_rect(
            bounds,
            self.paint.face.columns(),
            self.paint.face.rows().len(),
            row_index,
            state.horizontal.offset(),
            state.vertical.offset(),
            self.paint.face.metrics(),
        );
        let row = visible.unwrap_or(Rect {
            h: 0.0,
            w: 0.0,
            x: bounds.x,
            y: bounds.y,
        });
        let hit = Hit::new(point, row);
        let was_pressed = state.pressed_index == Some(row_index);
        let outcome = state.row_drag.on_input(input, &hit);
        let released = matches!(
            input,
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Up
        );
        if released {
            state.drag_index = None;
            state.pressed_index = None;
        }
        let action = drag(&self.paint.path, row_index, outcome);
        if action.is_some() {
            return action;
        }
        if matches!(
            input,
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Down
        ) {
            return index(&self.paint.path, Outcome::captured());
        }
        if released && was_pressed {
            return if hit.over() {
                index(&self.paint.path, Outcome::set(row_index))
            } else {
                Some(Action::request_redraw().and_capture())
            };
        }
        None
    }
}

fn scroll_action(
    path: &str,
    outcome: Outcome<usize>,
    before: f32,
    after: f32,
) -> Option<Action<Published>> {
    if outcome.is_captured() && outcome.value().is_none() && before != after {
        Some(Action::request_redraw().and_capture())
    } else {
        index(path, outcome)
    }
}

fn divider_drag(track: Track) -> Scalar {
    Scalar::builder()
        .track(track)
        .hover(Hover::new(CursorShape::ResizeH))
        .build()
}
