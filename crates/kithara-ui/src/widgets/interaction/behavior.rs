use std::mem;

use iced::{
    Event, Point, Rectangle,
    mouse::{self, Button, Cursor, ScrollDelta},
    time::Instant,
    widget::canvas::Action,
};

use crate::render::{ControlAction, DragPhase, UiEvent};

#[derive(Clone, Copy)]
pub(crate) struct HoverState {
    interaction: mouse::Interaction,
}

impl HoverState {
    pub(crate) const fn new(interaction: mouse::Interaction) -> Self {
        Self { interaction }
    }

    pub(crate) fn interaction(
        self,
        active: bool,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if active || cursor.is_over(bounds) {
            self.interaction
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Drag source for one item of a list. It watches the pointer without ever
/// capturing it, so the item keeps its own click behaviour and every other
/// control still sees the same events.
pub(crate) struct ItemDrag {
    path: String,
    index: usize,
}

#[derive(Default)]
pub(crate) struct ItemDragState {
    held: bool,
    origin: Option<Point>,
    active: bool,
}

impl ItemDragState {
    pub(crate) const fn interaction(&self) -> mouse::Interaction {
        if self.active {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::None
        }
    }
}

impl ItemDrag {
    /// Pointer travel that turns a press on an item into a drag; below it the
    /// press stays a plain click.
    const DRAG_THRESHOLD: f32 = 4.0;

    pub(crate) const fn new(path: String, index: usize) -> Self {
        Self { path, index }
    }

    fn publish(&self, phase: DragPhase) -> Action<UiEvent> {
        Action::publish(UiEvent::Control {
            path: self.path.clone(),
            action: ControlAction::Drag(phase),
        })
    }

    pub(crate) fn update(
        &self,
        state: &mut ItemDragState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if cursor.is_over(bounds) => {
                *state = ItemDragState {
                    held: true,
                    ..ItemDragState::default()
                };
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.held && !state.active => {
                let Some(origin) = state.origin else {
                    state.origin = Some(*position);
                    return None;
                };
                if position.distance(origin) < Self::DRAG_THRESHOLD {
                    return None;
                }
                state.active = true;
                Some(self.publish(DragPhase::Start(self.index)))
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                let dragging = mem::take(state).active;
                dragging.then(|| self.publish(DragPhase::Drop))
            }
            _ => None,
        }
    }
}

#[derive(bon::Builder)]
pub(crate) struct HorizontalPixelDrag {
    path: String,
    value: f32,
    minimum: f32,
    hover: HoverState,
}

#[derive(Default)]
pub(crate) struct HorizontalPixelDragState {
    active: bool,
    start_position: f32,
    start_value: f32,
}

impl HorizontalPixelDrag {
    pub(crate) fn mouse_interaction(
        &self,
        state: &HorizontalPixelDragState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        self.hover.interaction(state.active, bounds, cursor)
    }

    fn publish(&self, value: f32) -> Action<UiEvent> {
        Action::publish(UiEvent::Control {
            path: self.path.clone(),
            action: ControlAction::SetScalar(f64::from(value)),
        })
        .and_capture()
    }

    pub(crate) fn update(
        &self,
        state: &mut HorizontalPixelDragState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if cursor.is_over(bounds) => {
                state.start_position = cursor.position()?.x;
                state.start_value = self.value;
                state.active = true;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.active => {
                cursor.position().map(|position| {
                    self.publish(
                        (state.start_value + position.x - state.start_position).max(self.minimum),
                    )
                })
            }
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) if state.active => {
                state.active = false;
                Some(Action::capture())
            }
            _ => None,
        }
    }
}

#[derive(bon::Builder)]
pub(crate) struct ClickActivate {
    path: String,
    hover: HoverState,
}

impl ClickActivate {
    pub(crate) fn mouse_interaction(
        &self,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        self.hover.interaction(false, bounds, cursor)
    }

    pub(crate) fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if cursor.is_over(bounds) => {
                Some(
                    Action::publish(UiEvent::Control {
                        path: self.path.clone(),
                        action: ControlAction::Activate,
                    })
                    .and_capture(),
                )
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ScalarDragMode {
    Horizontal,
    HorizontalClick,
    RelativeHorizontal { value: f32, scale: f32 },
    Vertical,
    RelativeVertical { value: f32, range: f32 },
}

/// Opt-in wheel stepping: the current normalized value plus the per-tick step.
#[derive(Clone, Copy)]
pub(crate) struct WheelStep {
    pub(crate) value: f32,
    pub(crate) step: f32,
}

#[derive(bon::Builder)]
pub(crate) struct ScalarDrag {
    path: String,
    mode: ScalarDragMode,
    hover: HoverState,
    double_click_value: Option<f32>,
    wheel: Option<WheelStep>,
}

#[derive(Default)]
pub(crate) struct DoubleClickState {
    previous: Option<(Point, Instant)>,
}

impl DoubleClickState {
    /// `as_millis` truncates, so the accepted window is `[0 ms, 301 ms)` rather
    /// than the 300 a `Duration` constant compared with `<=` would give. The
    /// boundary is unreachable from a test while `now` is read in here.
    pub(crate) fn register(&mut self, position: Point) -> bool {
        let now = Instant::now();
        let consecutive = self
            .previous
            .is_some_and(|(previous_position, previous_time)| {
                previous_position.distance(position) < 6.0
                    && now
                        .checked_duration_since(previous_time)
                        .is_some_and(|duration| duration.as_millis() <= 300)
            });
        self.previous = (!consecutive).then_some((position, now));
        consecutive
    }
}

#[derive(Default)]
pub(crate) struct ScalarDragState {
    active: bool,
    start_position: f32,
    start_value: f32,
    double_click: DoubleClickState,
    wheel_accum: f32,
}

impl ScalarDrag {
    fn absolute_action(&self, bounds: Rectangle, cursor: Cursor) -> Option<Action<UiEvent>> {
        let position = cursor.position()?;
        let value = match self.mode {
            ScalarDragMode::Horizontal | ScalarDragMode::HorizontalClick => {
                normalized_horizontal(bounds, position)?
            }
            ScalarDragMode::Vertical if bounds.height > 0.0 => {
                1.0 - (position.y - bounds.y) / bounds.height
            }
            ScalarDragMode::Vertical
            | ScalarDragMode::RelativeHorizontal { .. }
            | ScalarDragMode::RelativeVertical { .. } => return None,
        };
        Some(self.publish(value.clamp(0.0, 1.0)))
    }

    pub(crate) fn mouse_interaction(
        &self,
        state: &ScalarDragState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        self.hover.interaction(state.active, bounds, cursor)
    }

    fn publish(&self, value: f32) -> Action<UiEvent> {
        Action::publish(UiEvent::Control {
            path: self.path.clone(),
            action: ControlAction::SetScalar(f64::from(value)),
        })
        .and_capture()
    }

    pub(crate) fn publish_child(&self, child: &str, value: f32) -> Action<UiEvent> {
        Action::publish(UiEvent::Control {
            path: format!("{}/{child}", self.path),
            action: ControlAction::SetScalar(f64::from(value)),
        })
        .and_capture()
    }

    fn double_click_action(
        &self,
        state: &mut ScalarDragState,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let value = self.double_click_value?;
        state
            .double_click
            .register(cursor.position()?)
            .then(|| self.publish(value))
    }

    pub(crate) fn update(
        &self,
        state: &mut ScalarDragState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if cursor.is_over(bounds) => {
                if let Some(action) = self.double_click_action(state, cursor) {
                    return Some(action);
                }
                match self.mode {
                    ScalarDragMode::RelativeHorizontal { value, .. } => {
                        state.start_position = cursor.position()?.x;
                        state.start_value = value;
                        state.active = true;
                        Some(Action::capture())
                    }
                    ScalarDragMode::RelativeVertical { value, .. } => {
                        state.start_position = cursor.position()?.y;
                        state.start_value = value;
                        state.active = true;
                        Some(Action::capture())
                    }
                    ScalarDragMode::Horizontal | ScalarDragMode::Vertical => {
                        state.active = true;
                        self.absolute_action(bounds, cursor)
                    }
                    ScalarDragMode::HorizontalClick => self.absolute_action(bounds, cursor),
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.active => match self.mode {
                ScalarDragMode::RelativeHorizontal { scale, .. } if bounds.width > 0.0 => {
                    cursor.position().map(|position| {
                        self.publish(
                            (state.start_value
                                - (position.x - state.start_position) / bounds.width * scale)
                                .clamp(0.0, 1.0),
                        )
                    })
                }
                ScalarDragMode::RelativeVertical { range, .. } => {
                    cursor.position().map(|position| {
                        self.publish(
                            (state.start_value + (state.start_position - position.y) / range)
                                .clamp(0.0, 1.0),
                        )
                    })
                }
                ScalarDragMode::Horizontal | ScalarDragMode::Vertical => {
                    self.absolute_action(bounds, cursor)
                }
                ScalarDragMode::HorizontalClick | ScalarDragMode::RelativeHorizontal { .. } => None,
            },
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) if state.active => {
                state.active = false;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let wheel = self.wheel?;
                let steps = wheel_steps(&mut state.wheel_accum, *delta);
                if steps == 0.0 {
                    return Some(Action::capture());
                }
                let value = wheel.step.mul_add(steps, wheel.value);
                Some(self.publish(value.clamp(0.0, 1.0)))
            }
            _ => None,
        }
    }
}

/// Scroll deltas arrive content-directed (macOS natural scrolling): an
/// upward gesture is negative, so the value axis is the negated delta.
fn wheel_steps(accum: &mut f32, delta: ScrollDelta) -> f32 {
    /// Trackpad pixel deltas per emitted wheel step. A discrete wheel detent
    /// (`ScrollDelta::Lines`) is always one step; trackpads stream many small
    /// `Pixels` events per gesture, so those accumulate to this threshold.
    const WHEEL_PIXELS_PER_STEP: f32 = 20.0;

    match delta {
        ScrollDelta::Lines { y, .. } => {
            if y == 0.0 {
                return 0.0;
            }
            -y.signum()
        }
        ScrollDelta::Pixels { y, .. } => {
            *accum -= y;
            let steps = (*accum / WHEEL_PIXELS_PER_STEP).trunc();
            *accum -= steps * WHEEL_PIXELS_PER_STEP;
            steps
        }
    }
}

pub(crate) fn scroll_y(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Lines { y, .. } | ScrollDelta::Pixels { y, .. } => y,
    }
}

fn normalized_horizontal(bounds: Rectangle, position: Point) -> Option<f32> {
    (bounds.width > 0.0).then(|| ((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn horizontal_click_maps_to_normalized_position() {
        let bounds = Rectangle::new(Point::new(20.0, 4.0), iced::Size::new(200.0, 40.0));

        for (x, expected) in [
            (0.0, 0.0),
            (20.0, 0.0),
            (120.0, 0.5),
            (220.0, 1.0),
            (260.0, 1.0),
        ] {
            assert_eq!(
                normalized_horizontal(bounds, Point::new(x, 10.0)),
                Some(expected)
            );
        }
    }

    const ROW: Rectangle = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 26.0,
    };

    fn moved(x: f32) -> Event {
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(x, 13.0),
        })
    }

    fn press() -> Event {
        Event::Mouse(mouse::Event::ButtonPressed(Button::Left))
    }

    fn release() -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(Button::Left))
    }

    fn dragged(path: &str, phase: DragPhase) -> UiEvent {
        UiEvent::Control {
            path: path.to_owned(),
            action: ControlAction::Drag(phase),
        }
    }

    fn moved_to(position: Point) -> Event {
        Event::Mouse(mouse::Event::CursorMoved { position })
    }

    fn scalar(path: &str, value: f64) -> UiEvent {
        UiEvent::Control {
            path: path.to_owned(),
            action: ControlAction::SetScalar(value),
        }
    }

    /// The VU meter's bounds: offset from the origin, so an inverted axis that
    /// forgot to subtract `bounds.y` would still look right at the top edge.
    fn vu() -> Rectangle {
        Rectangle::new(Point::new(0.0, 10.0), iced::Size::new(12.0, 40.0))
    }

    fn knob() -> Rectangle {
        Rectangle::new(Point::ORIGIN, iced::Size::new(34.0, 34.0))
    }

    fn vu_drag() -> ScalarDrag {
        ScalarDrag::builder()
            .path("vu".to_owned())
            .mode(ScalarDragMode::Vertical)
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .build()
    }

    /// `range` is a power of two so the expected values are exact in binary and
    /// the assertions can be equalities rather than tolerances.
    fn knob_drag(value: f32) -> ScalarDrag {
        ScalarDrag::builder()
            .path("knob".to_owned())
            .mode(ScalarDragMode::RelativeVertical {
                value,
                range: 128.0,
            })
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .build()
    }

    fn resetting_knob_drag(reset: f32) -> ScalarDrag {
        ScalarDrag::builder()
            .path("knob".to_owned())
            .mode(ScalarDragMode::RelativeVertical {
                value: 0.8,
                range: 128.0,
            })
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .double_click_value(reset)
            .build()
    }

    #[kithara::test]
    fn vertical_drag_publishes_the_inverted_position() {
        let drag = vu_drag();

        // Every fraction here is dyadic, so `f64::from` widens the `f32` result
        // without a residue and the assertion can be an equality.
        for (y, expected) in [(10.0, 1.0), (20.0, 0.75), (30.0, 0.5), (40.0, 0.25)] {
            let mut state = ScalarDragState::default();
            let cursor = Cursor::Available(Point::new(6.0, y));

            let (message, _, _) = drag
                .update(&mut state, &press(), vu(), cursor)
                .unwrap()
                .into_inner();

            assert_eq!(message, Some(scalar("vu", expected)), "at y={y}");
        }
    }

    #[kithara::test]
    fn vertical_drag_on_zero_height_publishes_nothing() {
        let drag = vu_drag();
        let cursor = Cursor::Available(Point::new(6.0, 30.0));
        let mut state = ScalarDragState::default();
        drag.update(&mut state, &press(), vu(), cursor).unwrap();

        let flattened = Rectangle {
            height: 0.0,
            ..vu()
        };

        assert!(
            drag.update(
                &mut state,
                &moved_to(Point::new(6.0, 30.0)),
                flattened,
                cursor
            )
            .is_none(),
            "a degenerate height must publish nothing rather than a clamped zero"
        );
    }

    #[kithara::test]
    fn vertical_drag_keeps_publishing_after_the_pointer_leaves() {
        let drag = vu_drag();
        let mut state = ScalarDragState::default();
        drag.update(
            &mut state,
            &press(),
            vu(),
            Cursor::Available(Point::new(6.0, 30.0)),
        )
        .unwrap();

        let escaped = Cursor::Available(Point::new(6.0, 200.0));
        let (message, _, _) = drag
            .update(&mut state, &moved_to(Point::new(6.0, 200.0)), vu(), escaped)
            .unwrap()
            .into_inner();

        assert_eq!(message, Some(scalar("vu", 0.0)));
    }

    #[kithara::test]
    fn vertical_press_publishes_and_captures_and_release_captures_silently() {
        let drag = vu_drag();
        let cursor = Cursor::Available(Point::new(6.0, 30.0));
        let mut state = ScalarDragState::default();

        let (pressed, _, press_status) = drag
            .update(&mut state, &press(), vu(), cursor)
            .unwrap()
            .into_inner();
        let (released, _, release_status) = drag
            .update(&mut state, &release(), vu(), cursor)
            .unwrap()
            .into_inner();

        assert_eq!(pressed, Some(scalar("vu", 0.5)));
        assert_eq!(press_status, iced::event::Status::Captured);
        assert_eq!(released, None);
        assert_eq!(release_status, iced::event::Status::Captured);
    }

    #[kithara::test]
    fn relative_vertical_drag_is_up_positive_and_scaled_by_range() {
        let drag = knob_drag(0.5);

        for (from, to, expected) in [(33.0, 1.0, 0.75), (1.0, 33.0, 0.25)] {
            let mut state = ScalarDragState::default();
            drag.update(
                &mut state,
                &press(),
                knob(),
                Cursor::Available(Point::new(17.0, from)),
            )
            .unwrap();

            let moved = Point::new(17.0, to);
            let (message, _, _) = drag
                .update(
                    &mut state,
                    &moved_to(moved),
                    knob(),
                    Cursor::Available(moved),
                )
                .unwrap()
                .into_inner();

            assert_eq!(message, Some(scalar("knob", expected)), "{from} -> {to}");
        }
    }

    #[kithara::test]
    fn relative_vertical_press_captures_without_publishing() {
        let drag = knob_drag(0.5);
        let cursor = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();

        let (message, _, status) = drag
            .update(&mut state, &press(), knob(), cursor)
            .unwrap()
            .into_inner();

        assert_eq!(message, None, "a relative press seeks nothing");
        assert_eq!(status, iced::event::Status::Captured);
    }

    #[kithara::test]
    fn double_click_slop_excludes_exactly_six_pixels() {
        for (separation, resets) in [(5.0, true), (6.0, false)] {
            let drag = resetting_knob_drag(0.5);
            let first = Cursor::Available(Point::new(17.0, 17.0));
            let second = Cursor::Available(Point::new(17.0, 17.0 + separation));
            let mut state = ScalarDragState::default();

            drag.update(&mut state, &press(), knob(), first).unwrap();
            drag.update(&mut state, &release(), knob(), first).unwrap();
            let (message, _, _) = drag
                .update(&mut state, &press(), knob(), second)
                .unwrap()
                .into_inner();

            assert_eq!(
                message.is_some(),
                resets,
                "a {separation} px separation must {} reset",
                if resets { "" } else { "not" }
            );
        }
    }

    #[kithara::test]
    fn a_reset_never_becomes_a_drag() {
        let drag = resetting_knob_drag(0.5);
        let cursor = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();

        drag.update(&mut state, &press(), knob(), cursor).unwrap();
        drag.update(&mut state, &release(), knob(), cursor).unwrap();
        drag.update(&mut state, &press(), knob(), cursor).unwrap();

        let moved = Point::new(17.0, 1.0);
        assert!(
            drag.update(
                &mut state,
                &moved_to(moved),
                knob(),
                Cursor::Available(moved)
            )
            .is_none(),
            "the press that reset the value must not have armed a drag"
        );
    }

    #[kithara::test]
    fn the_release_after_a_reset_is_not_captured() {
        let drag = resetting_knob_drag(0.5);
        let cursor = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();

        drag.update(&mut state, &press(), knob(), cursor).unwrap();
        drag.update(&mut state, &release(), knob(), cursor).unwrap();
        drag.update(&mut state, &press(), knob(), cursor).unwrap();

        assert!(
            drag.update(&mut state, &release(), knob(), cursor)
                .is_none(),
            "no gesture is active, so the release belongs to whoever is behind"
        );
    }

    #[kithara::test]
    fn a_double_click_pair_is_spent() {
        let drag = resetting_knob_drag(0.5);
        let cursor = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();

        for _ in 0..2 {
            drag.update(&mut state, &press(), knob(), cursor).unwrap();
            drag.update(&mut state, &release(), knob(), cursor);
        }

        let (message, _, _) = drag
            .update(&mut state, &press(), knob(), cursor)
            .unwrap()
            .into_inner();

        assert_eq!(
            message, None,
            "the third press starts a fresh pair rather than resetting again"
        );
    }

    #[kithara::test]
    fn the_cursor_shape_follows_hover_or_an_active_gesture() {
        let drag = knob_drag(0.5);
        let over = Cursor::Available(Point::new(17.0, 17.0));
        let away = Cursor::Available(Point::new(200.0, 200.0));
        let mut state = ScalarDragState::default();

        assert_eq!(
            drag.mouse_interaction(&state, knob(), over),
            mouse::Interaction::ResizingVertically
        );
        assert_eq!(
            drag.mouse_interaction(&state, knob(), away),
            mouse::Interaction::None
        );

        drag.update(&mut state, &press(), knob(), over).unwrap();

        assert_eq!(
            drag.mouse_interaction(&state, knob(), away),
            mouse::Interaction::ResizingVertically,
            "an active gesture keeps its shape once the pointer leaves"
        );
    }

    #[kithara::test]
    fn item_drag_starts_past_the_threshold_and_ends_on_release() {
        let drag = ItemDrag::new("library/tracks".to_owned(), 3);
        let at = |x: f32| Cursor::Available(Point::new(x, 13.0));
        let mut state = ItemDragState::default();

        assert!(drag.update(&mut state, &press(), ROW, at(10.0)).is_none());
        assert!(
            drag.update(&mut state, &moved(11.0), ROW, at(11.0))
                .is_none(),
            "the first move only fixes the point the travel is measured from"
        );
        assert!(
            drag.update(&mut state, &moved(13.0), ROW, at(13.0))
                .is_none(),
            "travel below the threshold stays a click"
        );

        let started = drag
            .update(&mut state, &moved(40.0), ROW, at(40.0))
            .unwrap_or_else(|| panic!("crossing the threshold must start the drag"));
        let (message, _, status) = started.into_inner();
        assert_eq!(
            message,
            Some(dragged("library/tracks", DragPhase::Start(3)))
        );
        assert_eq!(status, iced::event::Status::Ignored);
        assert!(
            drag.update(&mut state, &moved(80.0), ROW, at(80.0))
                .is_none(),
            "a drag starts once"
        );

        let dropped = drag
            .update(&mut state, &release(), ROW, at(80.0))
            .unwrap_or_else(|| panic!("release must end the drag"));
        assert_eq!(
            dropped.into_inner().0,
            Some(dragged("library/tracks", DragPhase::Drop))
        );
    }

    #[kithara::test]
    fn item_drag_starts_after_the_pointer_has_left_the_item() {
        let drag = ItemDrag::new("library/tracks".to_owned(), 1);
        let gone = Cursor::Unavailable;
        let mut state = ItemDragState::default();

        assert!(
            drag.update(
                &mut state,
                &press(),
                ROW,
                Cursor::Available(Point::new(8.0, 13.0))
            )
            .is_none()
        );
        assert!(drag.update(&mut state, &moved(300.0), ROW, gone).is_none());

        let started = drag
            .update(&mut state, &moved(340.0), ROW, gone)
            .unwrap_or_else(|| panic!("the drag must start away from the item"));
        assert_eq!(
            started.into_inner().0,
            Some(dragged("library/tracks", DragPhase::Start(1)))
        );

        let dropped = drag
            .update(&mut state, &release(), ROW, gone)
            .unwrap_or_else(|| panic!("release must end the drag"));
        assert_eq!(
            dropped.into_inner().0,
            Some(dragged("library/tracks", DragPhase::Drop))
        );
    }

    #[kithara::test]
    fn a_press_restarts_the_gesture() {
        let drag = ItemDrag::new("library/tracks".to_owned(), 2);
        let at = |x: f32| Cursor::Available(Point::new(x, 13.0));
        let mut state = ItemDragState::default();

        assert!(drag.update(&mut state, &press(), ROW, at(10.0)).is_none());
        assert!(
            drag.update(&mut state, &moved(10.0), ROW, at(10.0))
                .is_none()
        );
        assert!(
            drag.update(&mut state, &moved(60.0), ROW, at(60.0))
                .is_some()
        );

        assert!(drag.update(&mut state, &press(), ROW, at(10.0)).is_none());
        assert!(
            drag.update(&mut state, &release(), ROW, at(10.0)).is_none(),
            "the new gesture never became a drag, so its release is a click"
        );
    }

    #[kithara::test]
    fn item_drag_is_silent_without_a_press_on_the_item() {
        let drag = ItemDrag::new("library/tracks".to_owned(), 0);
        let away = Cursor::Available(Point::new(600.0, 13.0));
        let mut state = ItemDragState::default();

        for event in [press(), moved(620.0), release()] {
            assert!(drag.update(&mut state, &event, ROW, away).is_none());
        }
    }

    #[kithara::test]
    fn horizontal_drag_publishes_normalized_scalar() {
        let drag = ScalarDrag::builder()
            .path("volume".to_owned())
            .mode(ScalarDragMode::Horizontal)
            .hover(HoverState::new(mouse::Interaction::ResizingHorizontally))
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(64.0, 22.0));
        let cursor = Cursor::Available(Point::new(16.0, 11.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let mut state = ScalarDragState::default();

        let action = drag
            .update(&mut state, &press, bounds, cursor)
            .unwrap_or_else(|| panic!("horizontal drag must publish an action"));
        let (message, _, _) = action.into_inner();

        assert_eq!(
            message,
            Some(UiEvent::Control {
                path: "volume".to_owned(),
                action: ControlAction::SetScalar(0.25),
            })
        );
    }

    #[kithara::test]
    fn relative_drag_double_click_resets_to_configured_value() {
        let drag = ScalarDrag::builder()
            .path("knob".to_owned())
            .mode(ScalarDragMode::RelativeVertical {
                value: 0.8,
                range: 140.0,
            })
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .double_click_value(0.5)
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(34.0, 34.0));
        let cursor = Cursor::Available(Point::new(17.0, 17.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(Button::Left));
        let mut state = ScalarDragState::default();

        assert!(drag.update(&mut state, &press, bounds, cursor).is_some());
        assert!(drag.update(&mut state, &release, bounds, cursor).is_some());
        let action = drag.update(&mut state, &press, bounds, cursor).unwrap();
        let (message, _, _) = action.into_inner();

        assert_eq!(
            message,
            Some(UiEvent::Control {
                path: "knob".to_owned(),
                action: ControlAction::SetScalar(0.5),
            })
        );
    }

    #[kithara::test]
    fn relative_horizontal_drag_uses_start_value_without_click_seek() {
        let drag = ScalarDrag::builder()
            .path("wave".to_owned())
            .mode(ScalarDragMode::RelativeHorizontal {
                value: 0.4,
                scale: 0.2,
            })
            .hover(HoverState::new(mouse::Interaction::Grab))
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(200.0, 40.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(Button::Left));
        let mut state = ScalarDragState::default();

        let pressed = drag
            .update(
                &mut state,
                &press,
                bounds,
                Cursor::Available(Point::new(100.0, 20.0)),
            )
            .unwrap_or_else(|| panic!("relative drag must capture its press"));
        assert_eq!(pressed.into_inner().0, None);

        let moved = drag
            .update(
                &mut state,
                &Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(150.0, 20.0),
                }),
                bounds,
                Cursor::Available(Point::new(150.0, 20.0)),
            )
            .unwrap_or_else(|| panic!("relative drag must publish movement"));
        let Some(UiEvent::Control {
            path,
            action: ControlAction::SetScalar(value),
        }) = moved.into_inner().0
        else {
            panic!("relative drag must publish a scalar");
        };
        assert_eq!(path, "wave");
        assert!((value - 0.35).abs() < 0.000_1);

        let released = drag
            .update(
                &mut state,
                &release,
                bounds,
                Cursor::Available(Point::new(150.0, 20.0)),
            )
            .unwrap_or_else(|| panic!("relative drag must capture its release"));
        assert_eq!(released.into_inner().0, None);
    }

    #[kithara::test]
    fn wheel_steps_the_value_by_direction_and_clamps() {
        let drag = ScalarDrag::builder()
            .path("knob".to_owned())
            .mode(ScalarDragMode::RelativeVertical {
                value: 0.5,
                range: 140.0,
            })
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .wheel(WheelStep {
                value: 0.5,
                step: 0.25,
            })
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(34.0, 34.0));
        let over = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();
        let wheel = |y: f32| {
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: ScrollDelta::Lines { x: 0.0, y },
            })
        };

        let up = drag.update(&mut state, &wheel(-1.0), bounds, over).unwrap();
        assert_eq!(
            up.into_inner().0,
            Some(UiEvent::Control {
                path: "knob".to_owned(),
                action: ControlAction::SetScalar(0.75),
            })
        );

        let down = drag.update(&mut state, &wheel(1.0), bounds, over).unwrap();
        assert_eq!(
            down.into_inner().0,
            Some(UiEvent::Control {
                path: "knob".to_owned(),
                action: ControlAction::SetScalar(0.25),
            })
        );

        assert!(
            drag.update(&mut state, &wheel(0.0), bounds, over)
                .is_some_and(|action| action.into_inner().0.is_none()),
            "zero delta must still capture over an opted-in control"
        );
        let away = Cursor::Available(Point::new(100.0, 100.0));
        assert!(drag.update(&mut state, &wheel(1.0), bounds, away).is_none());
    }

    #[kithara::test]
    fn trackpad_pixels_accumulate_to_whole_steps() {
        let drag = ScalarDrag::builder()
            .path("knob".to_owned())
            .mode(ScalarDragMode::RelativeVertical {
                value: 0.5,
                range: 140.0,
            })
            .hover(HoverState::new(mouse::Interaction::ResizingVertically))
            .wheel(WheelStep {
                value: 0.5,
                step: 0.25,
            })
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(34.0, 34.0));
        let over = Cursor::Available(Point::new(17.0, 17.0));
        let mut state = ScalarDragState::default();
        let pixels = |y: f32| {
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: ScrollDelta::Pixels { x: 0.0, y },
            })
        };

        let below_threshold = drag
            .update(&mut state, &pixels(-12.0), bounds, over)
            .unwrap();
        assert_eq!(
            below_threshold.into_inner().0,
            None,
            "sub-threshold pixels capture without publishing"
        );

        let crossed = drag
            .update(&mut state, &pixels(-12.0), bounds, over)
            .unwrap();
        assert_eq!(
            crossed.into_inner().0,
            Some(UiEvent::Control {
                path: "knob".to_owned(),
                action: ControlAction::SetScalar(0.75),
            })
        );

        let flick = drag
            .update(&mut state, &pixels(45.0), bounds, over)
            .unwrap();
        assert_eq!(
            flick.into_inner().0,
            Some(UiEvent::Control {
                path: "knob".to_owned(),
                action: ControlAction::SetScalar(0.0),
            })
        );
    }

    #[kithara::test]
    fn wheel_is_inert_without_an_opt_in() {
        let drag = ScalarDrag::builder()
            .path("wave".to_owned())
            .mode(ScalarDragMode::HorizontalClick)
            .hover(HoverState::new(mouse::Interaction::Pointer))
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(200.0, 40.0));
        let mut state = ScalarDragState::default();
        let wheel = Event::Mouse(mouse::Event::WheelScrolled {
            delta: ScrollDelta::Lines { x: 0.0, y: 1.0 },
        });

        assert!(
            drag.update(
                &mut state,
                &wheel,
                bounds,
                Cursor::Available(Point::new(10.0, 10.0)),
            )
            .is_none()
        );
    }

    #[kithara::test]
    fn horizontal_pixel_drag_uses_start_width_and_clamps_minimum() {
        let drag = HorizontalPixelDrag::builder()
            .path("tracklist/table/width/artist".to_owned())
            .value(180.0)
            .minimum(28.0)
            .hover(HoverState::new(mouse::Interaction::ResizingHorizontally))
            .build();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(7.0, 22.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let mut state = HorizontalPixelDragState::default();

        assert!(
            drag.update(
                &mut state,
                &press,
                bounds,
                Cursor::Available(Point::new(3.0, 11.0)),
            )
            .is_some()
        );
        let grown = drag
            .update(
                &mut state,
                &Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(43.0, 11.0),
                }),
                bounds,
                Cursor::Available(Point::new(43.0, 11.0)),
            )
            .unwrap_or_else(|| panic!("pixel drag must publish movement"));
        assert_eq!(
            grown.into_inner().0,
            Some(UiEvent::Control {
                path: "tracklist/table/width/artist".to_owned(),
                action: ControlAction::SetScalar(220.0),
            })
        );

        let clamped = drag
            .update(
                &mut state,
                &Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(-300.0, 11.0),
                }),
                bounds,
                Cursor::Available(Point::new(-300.0, 11.0)),
            )
            .unwrap_or_else(|| panic!("pixel drag must publish clamped movement"));
        assert_eq!(
            clamped.into_inner().0,
            Some(UiEvent::Control {
                path: "tracklist/table/width/artist".to_owned(),
                action: ControlAction::SetScalar(28.0),
            })
        );
    }
}
