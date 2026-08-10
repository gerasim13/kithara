use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use masonry::{
    core::{PointerEvent, WidgetId},
    kurbo::Rect as MasonryRect,
};
use num_traits::cast::AsPrimitive;

use super::{custom::HostAction, node::pointer_button};
use crate::{
    atoms::{track_list::face::Drawn, tree::retained::Drawn as TreeDrawn},
    draw::{Pt, Rect},
    engine::{Engine, Target},
    interact::{CursorShape, Input, MOUSE, Outcome, PointerInput, PointerPhase},
    render::{
        HostedControlPlan, UiEvent, engine_value,
        hosted::{TrackListPlan, TrackListProjection, TreePlan, TreeProjection},
    },
};

/// One control an engine drives: what it is and where it sits.
pub(crate) struct EngineTarget {
    pub(super) area: Rc<Cell<MasonryRect>>,
    pub(super) plan: HostedControlPlan,
}

/// What routing one event through the engine produced.
pub(super) struct Routed {
    pub(super) focused: bool,
    pub(super) outcome: Outcome<HostAction>,
    pub(super) repaint: bool,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct HostedEngine {
    engine: Rc<RefCell<Engine>>,
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    #[field(get(copy), vis = "pub(super)")]
    owner: WidgetId,
    pointer: Rc<Cell<Option<Pt>>>,
    _projections: Vec<Rc<dyn TrackListProjection>>,
    _tree_projections: Vec<Rc<dyn TreeProjection>>,
    targets: Vec<EngineTarget>,
    #[field(get(copy), vis = "pub(super)", rename = "accepts_text_input")]
    text_input: bool,
}

impl HostedEngine {
    pub(super) fn new(
        owner: WidgetId,
        targets: Vec<EngineTarget>,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Rc<Self> {
        let text_input = targets
            .iter()
            .any(|target| matches!(target.plan, HostedControlPlan::Tree(_)));
        let mut engine = Engine::default();
        engine.reconcile(targets.iter().flat_map(|target| target.plan.descriptors()));
        let engine = Rc::new(RefCell::new(engine));
        let pointer = Rc::new(Cell::new(None));
        Rc::new_cyclic(|host| {
            let projections = targets
                .iter()
                .filter_map(|target| {
                    let HostedControlPlan::TrackList(plan) = &target.plan else {
                        return None;
                    };
                    let projection: Rc<dyn TrackListProjection> = Rc::new(EngineProjection {
                        area: Rc::clone(&target.area),
                        engine: Rc::clone(&engine),
                        host: host.clone(),
                        pointer: Rc::clone(&pointer),
                    });
                    plan.bind_projection(Rc::downgrade(&projection));
                    Some(projection)
                })
                .collect();
            let tree_projections = targets
                .iter()
                .filter_map(|target| {
                    let HostedControlPlan::Tree(plan) = &target.plan else {
                        return None;
                    };
                    let projection: Rc<dyn TreeProjection> = Rc::new(EngineProjection {
                        area: Rc::clone(&target.area),
                        engine: Rc::clone(&engine),
                        host: host.clone(),
                        pointer: Rc::clone(&pointer),
                    });
                    plan.bind_projection(Rc::downgrade(&projection));
                    Some(projection)
                })
                .collect();
            Self {
                engine: Rc::clone(&engine),
                map_event,
                owner,
                pointer: Rc::clone(&pointer),
                _projections: projections,
                _tree_projections: tree_projections,
                targets,
                text_input,
            }
        })
    }

    pub(super) fn route(&self, input: Input<'_>, point: Option<Pt>) -> Routed {
        let mut engine = self.engine.borrow_mut();
        let before = self.track_list_views(&engine, self.pointer.get());
        let tree_before = self.tree_views(&engine, self.pointer.get());
        if matches!(input, Input::Pointer(_) | Input::Wheel(_)) {
            self.pointer.set(point);
        }
        let targets = self.targets(&engine, point);
        let descriptors = self
            .targets
            .iter()
            .flat_map(|target| target.plan.active_descriptors(&targets))
            .collect::<Vec<_>>();
        engine.reconcile(descriptors);
        for target in &targets {
            engine.set_scroll_viewport(target.path, target.hit.area());
        }
        let emission = engine.handle(input, &targets, kithara_platform::time::Instant::now());
        let focused = engine.focused_path().is_some();
        let repaint = before != self.track_list_views(&engine, self.pointer.get())
            || tree_before != self.tree_views(&engine, self.pointer.get());
        let Some(emission) = emission else {
            return Routed {
                focused,
                outcome: Outcome::IGNORED,
                repaint,
            };
        };
        let path = emission.path;
        let child = emission.child;
        let outcome = emission
            .outcome
            .map(|event| (self.map_event)(engine_value(&path, child, event)));
        Routed {
            focused,
            outcome,
            repaint,
        }
    }

    pub(super) fn input_method_area(&self) -> Option<Rect> {
        let engine = self.engine.borrow();
        let targets = self.targets(&engine, None);
        engine.input_method(&targets).map(|request| request.caret)
    }

    pub(super) fn cursor(&self, point: Pt) -> CursorShape {
        let engine = self.engine.borrow();
        let targets = self.targets(&engine, Some(point));
        engine.cursor(&targets)
    }

    pub(super) fn has_open_picker(&self) -> bool {
        let engine = self.engine.borrow();
        self.targets.iter().any(|target| {
            let HostedControlPlan::Picker { path, .. } = &target.plan else {
                return false;
            };
            engine
                .picker_snapshot(path)
                .is_some_and(|snapshot| snapshot.open)
        })
    }

    #[cfg(test)]
    pub(super) fn tree_picture(&self, path: &str) -> Option<(usize, String)> {
        self.targets.iter().find_map(|target| {
            let HostedControlPlan::Tree(plan) = &target.plan else {
                return None;
            };
            if plan.path != path {
                return None;
            }
            let picture = plan.picture();
            Some((picture.row_count(), picture.query().to_owned()))
        })
    }

    delegate::delegate! {
        to self.engine.borrow_mut() {
            pub(super) fn clear_focus(&self);
        }
    }

    fn targets<'a>(&'a self, engine: &Engine, point: Option<Pt>) -> Vec<Target<'a>> {
        let mut targets = Vec::new();
        for target in &self.targets {
            let area = target.area.get();
            target.plan.append_targets(
                Rect {
                    x: area.x0.as_(),
                    y: area.y0.as_(),
                    w: area.width().as_(),
                    h: area.height().as_(),
                },
                point,
                Some(engine),
                &mut targets,
            );
        }
        targets
    }

    fn track_list_views(&self, engine: &Engine, point: Option<Pt>) -> Vec<Drawn> {
        self.targets
            .iter()
            .filter_map(|target| {
                let HostedControlPlan::TrackList(plan) = &target.plan else {
                    return None;
                };
                plan.view(engine, point, target_bounds(target))
            })
            .collect()
    }

    fn tree_views(&self, engine: &Engine, point: Option<Pt>) -> Vec<TreeDrawn> {
        self.targets
            .iter()
            .filter_map(|target| {
                let HostedControlPlan::Tree(plan) = &target.plan else {
                    return None;
                };
                plan.view(engine, point, target_bounds(target))
            })
            .collect()
    }
}

struct EngineProjection {
    area: Rc<Cell<MasonryRect>>,
    engine: Rc<RefCell<Engine>>,
    host: Weak<HostedEngine>,
    pointer: Rc<Cell<Option<Pt>>>,
}

impl TrackListProjection for EngineProjection {
    fn project(&self, plan: &TrackListPlan) -> Option<Drawn> {
        let engine = self.engine.borrow();
        plan.view(&engine, self.pointer.get(), bounds(self.area.get()))
    }

    fn reconcile(&self) {
        self.reconcile_engine();
    }
}

impl TreeProjection for EngineProjection {
    fn project(&self, plan: &TreePlan) -> Option<TreeDrawn> {
        let engine = self.engine.borrow();
        plan.view(&engine, self.pointer.get(), bounds(self.area.get()))
    }

    fn reconcile(&self) {
        self.reconcile_engine();
    }
}

impl EngineProjection {
    fn reconcile_engine(&self) {
        if let Some(host) = self.host.upgrade() {
            host.engine.borrow_mut().reconcile(
                host.targets
                    .iter()
                    .flat_map(|target| target.plan.descriptors()),
            );
        }
    }
}

fn target_bounds(target: &EngineTarget) -> Rect {
    bounds(target.area.get())
}

fn bounds(area: MasonryRect) -> Rect {
    Rect {
        x: area.x0.as_(),
        y: area.y0.as_(),
        w: area.width().as_(),
        h: area.height().as_(),
    }
}

pub(super) fn input(event: &PointerEvent) -> Option<(Input<'static>, Pt)> {
    let (phase, button, state) = match event {
        PointerEvent::Down(button) => (
            PointerPhase::Down,
            button.button.map(pointer_button),
            &button.state,
        ),
        PointerEvent::Move(update) => (PointerPhase::Move, None, &update.current),
        _ => return None,
    };
    let position = state.logical_position();
    let point = Pt {
        x: position.x.as_(),
        y: position.y.as_(),
    };
    Some((
        Input::Pointer(PointerInput::new(
            MOUSE,
            button,
            phase,
            Some(point),
            state.count,
        )),
        point,
    ))
}
