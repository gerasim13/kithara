use std::rc::Rc;

use iced::advanced::{layout::Layout, mouse};

use super::{
    geometry::{Rendered, tree_input_layouts},
    mount::{Cx, ViewControl, painted},
    table::TableHost,
};
use crate::{
    atoms::{bar::context::Context, design::fader::rail_bounds},
    draw::{Rect, Transform},
    engine::{Descriptor, Engine, Target},
    expand::{Binding, ControlSpec},
    hosts::{
        controls::Draws,
        hosted::{HostedControlPlan, Resolving},
    },
    iced::paint::PainterLength,
    ids::InternId,
    interact::{Hit, iced as iced_interact},
    mount,
    render::{InputOwner, ReadValue, Skin, document::Ctx},
};

/// How the document placed one control: who owns its pointer, and the offset
/// every enclosing object folded into its box.
#[derive(Clone, Copy)]
pub(super) struct Placed {
    pub(super) owner: InputOwner,
    pub(super) transform: Transform,
}

pub(super) fn render_control<'a>(
    path: InternId,
    spec: &ControlSpec,
    read: Option<&Binding>,
    ctx: Ctx<'a, '_>,
    skin: &'a Skin,
    placed: Placed,
) -> Rendered<'a> {
    let value = read.and_then(|binding| ctx.read(binding));
    let path = ctx.ui.resolve(path);
    let cx = Cx {
        owner: placed.owner,
        transform: placed.transform,
        path,
        ctx,
        scope: ctx.scope(read),
        skin: skin.at(path),
        value: value.as_ref(),
    };
    mount::controls!(host: spec, Mount { cx: &cx })
}

/// Asks whichever control the document named to mount itself here.
struct Mount<'cx, 'a, 'ctx, 'value> {
    cx: &'cx Cx<'a, 'ctx, 'value>,
}

impl<'a> Mount<'_, 'a, '_, '_> {
    fn apply<C: ViewControl>(self, control: &C) -> Rendered<'a> {
        control.view(self.cx)
    }

    fn painted<C>(self, control: &C) -> Rendered<'a>
    where
        C: Draws,
        C::Painter: PainterLength + 'static,
    {
        painted(control, self.cx)
    }
}

pub(super) struct HostedControl {
    plan: HostedControlPlan,
    table: Option<Box<TableHost>>,
}

impl HostedControl {
    pub(super) fn new(
        path: &str,
        spec: &ControlSpec,
        value: Option<ReadValue<'_>>,
        read: Option<&Binding>,
        scope: &str,
        cx: Resolving<'_>,
    ) -> Option<Self> {
        HostedControlPlan::resolved(path, spec, value, read, scope, cx).map(Self::mounted)
    }

    /// Narrows a control's rectangle to the part a pointer actually drives. A
    /// fader is one canvas holding a caption and a rail; only the rail answers.
    fn input_bounds(&self, bounds: Rect) -> Rect {
        match &self.plan {
            HostedControlPlan::Fader {
                style,
                labelled,
                metrics,
                ..
            } => rail_bounds(bounds, *style, *labelled, *metrics),
            HostedControlPlan::Picker { face, .. } => Context::placed(*face, bounds),
            HostedControlPlan::Search(plan) => {
                crate::atoms::search::input_bounds(bounds, plan.picture.borrow().skin())
            }
            _ => bounds,
        }
    }

    pub(super) fn mounted(plan: HostedControlPlan) -> Self {
        let table = match &plan {
            HostedControlPlan::Table(plan) => Some(Box::new(TableHost::new(
                &plan.path,
                plan.columns(),
                plan.row_count(),
                plan.picture.borrow().metrics(),
                Rc::clone(&plan.viewport_width),
            ))),
            _ => None,
        };
        Self { plan, table }
    }

    pub(super) fn picker(&self) -> Option<(&str, usize, f32)> {
        match &self.plan {
            HostedControlPlan::Picker {
                path,
                items,
                item_height,
                ..
            } => Some((path, items.len(), *item_height)),
            _ => None,
        }
    }

    delegate::delegate! {
        to self.plan {
            fn path(&self) -> &str;
        }
    }
}

pub(super) fn append_control_targets<'a>(
    control: &'a HostedControl,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    engine: Option<&Engine>,
    targets: &mut Vec<Target<'a>>,
) {
    if let HostedControlPlan::Tree(plan) = &control.plan {
        let (search, rows) = tree_input_layouts(layout, plan.search_path.is_some());
        if let Some((path, layout)) = plan.search_path.as_ref().zip(search) {
            let input =
                crate::atoms::search::input_bounds(layout.bounds().into(), plan.picture().skin());
            targets.push(Target::new(
                path,
                Hit::new(cursor.position().map(Into::into), input),
            ));
        }
        if let Some(layout) = rows {
            targets.push(Target::new(
                &plan.path,
                iced_interact::hit(layout.bounds(), cursor),
            ));
            if let Some(offset) = engine.and_then(|engine| engine.scroll_offset(&plan.path)) {
                plan.append_toggle_targets(
                    layout.bounds().into(),
                    cursor.position().map(Into::into),
                    offset,
                    targets,
                );
            }
        }
        return;
    }
    if let Some(table) = &control.table {
        let actions = match &control.plan {
            HostedControlPlan::Table(plan) => Some(&**plan),
            _ => None,
        };
        table.append_targets(layout, cursor, engine, actions, targets);
    } else {
        targets.push(Target::new(
            control.path(),
            Hit::new(
                cursor.position().map(Into::into),
                control.input_bounds(layout.bounds().into()),
            ),
        ));
    }
}

pub(super) fn append_control_descriptors(
    control: &HostedControl,
    descriptors: &mut Vec<Descriptor>,
) {
    descriptors.extend(control.plan.descriptors());
}
