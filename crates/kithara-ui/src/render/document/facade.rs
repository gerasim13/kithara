use num_traits::cast::AsPrimitive;

use super::{Band, Ctx, Group, GroupMount, Host, Measured, Module, Popover, SplitMount};
use crate::{
    compile::{CompiledNode, CompiledUi},
    draw::Transform,
    expand::{Binding, ExpandedNode, MeasureSpec, SurfaceSpec},
    ids::InternId,
    layout::{Axis, FrameSides},
    module::{ChromeStyle, MeasureAxis, Motion, PopoverAlign, PopoverAt, Pose, TextAlign},
    render::{InputOwner, ReadValue},
    size::{
        Dim, SizeSpec, Snapshot, branch as adaptive_branch, compiled_node_size_with_hidden,
        effective_size, is_hidden, visible_compiled_children,
    },
    skin::{ColorRole, SkinDoc},
};

const HOSTED_MODULES: [&str; 27] = [
    "app-bar",
    "app-deck",
    "app-library",
    "app-menu",
    "app-menu-module-cell",
    "app-menu-window-row",
    "app-select-row",
    "app-strip",
    "app-strip-eq-3-band",
    "app-strip-eq-4-band",
    "app-mixer",
    "app-mixer-single",
    "app-overview",
    "app-overview-row",
    "app-overview-single",
    "gallery-knobs",
    "gallery-meters",
    "gallery-toggles",
    "gallery-chips",
    "gallery-buttons-tab",
    "gallery-cells-tab",
    "gallery-faders-tab",
    "gallery-library2-tab",
    "gallery-table-tab",
    "gallery-tree-tab",
    "gallery-module-tabs",
    "gallery-nav",
];

/// Produces a complete host output from a compiled document.
///
/// The traversal, conditional visibility, retained-owner selection, and root
/// composition are toolkit-neutral. `host` mounts each already-traversed node
/// into its local layout, paint, and interaction vocabulary.
#[must_use]
pub fn render<H>(node: &CompiledNode, ctx: Ctx<'_, '_>, mut host: H) -> H::Output
where
    H: Host,
{
    let content = compiled(node, ctx, &mut host);
    host.window(content, dragged_label(ctx), ctx.ui.resize_edges)
}

#[cfg(test)]
pub(crate) fn render_engine_subtree<H>(
    node: &ExpandedNode,
    address: &[usize],
    owner: InternId,
    ctx: Ctx<'_, '_>,
    mut host: H,
) -> H::Output
where
    H: Host,
{
    expanded(
        node,
        address,
        Branch {
            owner,
            input_owner: InputOwner::Engine,
            transform: Transform::IDENTITY,
        },
        ctx,
        &mut host,
    )
}

fn compiled<H>(node: &CompiledNode, ctx: Ctx<'_, '_>, host: &mut H) -> H::Output
where
    H: Host,
{
    let snapshot: &dyn Snapshot = &ctx;
    match node {
        CompiledNode::Optional { child, .. } => compiled(child, ctx, host),
        CompiledNode::Adaptive {
            axis,
            size,
            base,
            steps,
        } => {
            let mut branches = Vec::with_capacity(steps.len() + 1);
            branches.push(compiled(base, ctx, host));
            for (_, node) in steps {
                branches.push(compiled(node, ctx, host));
            }
            host.measured(
                Measured {
                    axis: *axis,
                    steps: steps.iter().map(|(from, _)| *from).collect(),
                    size: *size,
                },
                branches,
            )
        }
        CompiledNode::Split {
            axis,
            measure,
            children,
            ..
        } => {
            let mut mounted = Vec::with_capacity(children.len());
            for cell in visible_compiled_children(children, snapshot) {
                let size = compiled_node_size_with_hidden(&cell.node, ctx.skin, snapshot);
                let output = compiled(&cell.node, ctx, host);
                mounted.push(SplitMount {
                    band: Band::new(cell.from, cell.until),
                    weight: cell.weight,
                    size,
                    output,
                });
            }
            host.split(*axis, *measure, mounted)
        }
        CompiledNode::Module {
            instance,
            module,
            title,
            chip,
            assign,
            chrome,
            frame,
            corners,
            footer,
            drop,
            collapsed,
            root,
            ..
        } => {
            let collapsed = *chrome == ChromeStyle::Full
                && matches!(
                    ctx.get(ctx.ui.resolve(*collapsed)),
                    Some(ReadValue::Bool(true))
                );
            let footer = footer
                .as_ref()
                .and_then(|binding| ctx.read(binding))
                .and_then(|value| match value {
                    ReadValue::Text(text) => Some(text.to_owned()),
                    _ => None,
                });
            let content_hosted = HOSTED_MODULES.contains(&ctx.ui.resolve(*module));
            let chrome_hosted = *chrome == ChromeStyle::Full || drop.is_some();
            let content = (!collapsed).then(|| {
                let child = expanded(
                    root,
                    &[],
                    Branch {
                        owner: *instance,
                        input_owner: if content_hosted {
                            InputOwner::Engine
                        } else {
                            InputOwner::Leaf
                        },
                        // A module starts a fresh document: nothing outside it
                        // can pose what it draws.
                        transform: Transform::IDENTITY,
                    },
                    ctx,
                    host,
                );
                if content_hosted {
                    host.hosted(root, child)
                } else {
                    child
                }
            });
            host.module(
                Module {
                    instance: *instance,
                    module: *module,
                    title: *title,
                    chip: *chip,
                    assign,
                    chrome: *chrome,
                    frame: *frame,
                    corners: *corners,
                    footer,
                    drop: drop.as_ref(),
                    collapsed,
                    chrome_hosted,
                },
                content,
            )
        }
    }
}

#[derive(Clone, Copy)]
struct Branch {
    owner: InternId,
    input_owner: InputOwner,
    /// Every enclosing object's pose, composed and resolved for this frame.
    transform: Transform,
}

#[derive(Clone, Copy)]
struct PopoverNode<'a> {
    path: InternId,
    open: &'a Binding,
    at: PopoverAt,
    align: PopoverAlign,
    anchor: &'a ExpandedNode,
    content: &'a ExpandedNode,
    size: Option<SizeSpec>,
}

#[derive(Clone, Copy)]
struct RowNode<'a> {
    measure: Option<MeasureAxis>,
    gap: Option<f32>,
    align: TextAlign,
    pad: Option<f32>,
    pad_x: Option<f32>,
    pad_y: Option<f32>,
    frame: Option<FrameSides>,
    background: Option<ColorRole>,
    background_alpha: Option<f32>,
    active: Option<&'a Binding>,
    active_background: Option<ColorRole>,
    frame_color: Option<ColorRole>,
    active_frame_color: Option<ColorRole>,
    surface: Option<&'a SurfaceSpec>,
    size: Option<SizeSpec>,
}

fn row_group<'a>(node: RowNode<'a>, ctx: Ctx<'_, '_>) -> Group<'a> {
    let active = ctx.flag(node.active);
    let padding = node.pad.unwrap_or(ctx.skin.layout.grid_pad);
    let background = active
        .then_some(node.active_background)
        .flatten()
        .or(node.background);
    let frame_color = active
        .then_some(node.active_frame_color)
        .flatten()
        .or(node.frame_color)
        .unwrap_or(ctx.skin.divider.color);
    Group {
        axis: Axis::Horizontal,
        measure: node.measure,
        alignment: node.align,
        gap: node.gap.unwrap_or(ctx.skin.layout.grid_gap),
        padding_x: node.pad_x.unwrap_or(padding),
        padding_y: node.pad_y.unwrap_or(padding),
        frame: node.frame,
        background,
        background_alpha: node.background_alpha,
        frame_color,
        frame_width: ctx.skin.divider.width,
        surface: node.surface,
        size: node.size,
    }
}

/// Whether this node hands its input to a retained engine before it mounts.
///
/// The subtree under an engine is mounted the same way either way, so the
/// question is asked once here and the answer never reaches [`mounted`].
fn expanded<H>(
    node: &ExpandedNode,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    if branch.input_owner == InputOwner::Leaf && hosts_engine(ctx.ui, branch.owner, address) {
        let child = expanded(
            node,
            address,
            Branch {
                input_owner: InputOwner::Engine,
                ..branch
            },
            ctx,
            host,
        );
        return host.hosted(node, child);
    }
    mounted(node, address, branch, ctx, host)
}

/// An adaptive block: the branches it chooses between and the box it keeps.
struct Adaptive<'a> {
    node: &'a ExpandedNode,
    measure: &'a MeasureSpec,
    size: Option<SizeSpec>,
    base: &'a ExpandedNode,
    steps: &'a [(f32, ExpandedNode)],
}

/// How an adaptive block becomes host output.
///
/// An axis names a room only the layout pass knows, so every branch is mounted
/// and the host chooses. A measured reading is answered here.
fn mount_adaptive<H>(
    adaptive: &Adaptive<'_>,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let &Adaptive {
        node,
        measure,
        size,
        base,
        steps,
    } = adaptive;
    let snapshot: &dyn Snapshot = &ctx;
    match measure.axis() {
        Some(axis) => {
            let mut branches = Vec::with_capacity(steps.len() + 1);
            branches.push(expanded(
                base,
                &child_address(address, 0),
                branch,
                ctx,
                host,
            ));
            for (index, (_, node)) in steps.iter().enumerate() {
                branches.push(expanded(
                    node,
                    &child_address(address, index + 1),
                    branch,
                    ctx,
                    host,
                ));
            }
            host.measured(
                Measured {
                    axis,
                    steps: steps.iter().map(|(from, _)| *from).collect(),
                    size: size
                        .or_else(|| effective_size(node, ctx.skin, snapshot))
                        .unwrap_or(SizeSpec::FILL),
                },
                branches,
            )
        }
        None => expanded(
            adaptive_branch(measure, base, steps, snapshot),
            &child_address(address, 0),
            branch,
            ctx,
            host,
        ),
    }
}

/// How a row becomes host output.
fn mount_row<H>(
    node: &ExpandedNode,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let ExpandedNode::Row {
        measure,
        children,
        gap,
        align,
        pad,
        pad_x,
        pad_y,
        frame,
        background,
        background_alpha,
        active,
        active_background,
        frame_color,
        active_frame_color,
        surface,
        ..
    } = node
    else {
        unreachable!("mount_row is called only for a row")
    };
    let snapshot: &dyn Snapshot = &ctx;
    mount_group(
        row_group(
            RowNode {
                measure: *measure,
                gap: *gap,
                align: *align,
                pad: *pad,
                pad_x: *pad_x,
                pad_y: *pad_y,
                frame: *frame,
                background: *background,
                background_alpha: *background_alpha,
                active: active.as_ref(),
                active_background: *active_background,
                frame_color: *frame_color,
                active_frame_color: *active_frame_color,
                surface: surface.as_ref(),
                size: effective_size(node, ctx.skin, snapshot),
            },
            ctx,
        ),
        children,
        address,
        branch,
        snapshot,
        ctx,
        host,
    )
}

/// How a column becomes host output.
fn mount_column<H>(
    node: &ExpandedNode,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let ExpandedNode::Column {
        measure,
        children,
        gap,
        align,
        pad,
        pad_x,
        pad_y,
        frame,
        background,
        background_alpha,
        surface,
        ..
    } = node
    else {
        unreachable!("mount_column is called only for a column")
    };
    let snapshot: &dyn Snapshot = &ctx;
    mount_group(
        Group {
            axis: Axis::Vertical,
            measure: *measure,
            alignment: *align,
            gap: gap.unwrap_or(ctx.skin.layout.grid_gap),
            padding_x: pad_x.unwrap_or(pad.unwrap_or(ctx.skin.layout.grid_pad)),
            padding_y: pad_y.unwrap_or(pad.unwrap_or(ctx.skin.layout.grid_pad)),
            frame: *frame,
            background: *background,
            background_alpha: *background_alpha,
            frame_color: ctx.skin.divider.color,
            frame_width: ctx.skin.divider.width,
            surface: surface.as_ref(),
            size: effective_size(node, ctx.skin, snapshot),
        },
        children,
        address,
        branch,
        snapshot,
        ctx,
        host,
    )
}

/// How one node becomes host output, once the engine question is settled.
fn mounted<H>(
    node: &ExpandedNode,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let snapshot: &dyn Snapshot = &ctx;
    match node {
        // A reveal says when its child stands, not what it is: the band is read
        // by the flow that holds it, and the child mounts as itself.
        ExpandedNode::Optional { child, .. } | ExpandedNode::Reveal { child, .. } => {
            expanded(child, &child_address(address, 0), branch, ctx, host)
        }
        ExpandedNode::Adaptive {
            measure,
            size,
            base,
            steps,
        } => mount_adaptive(
            &Adaptive {
                node,
                measure,
                size: *size,
                base,
                steps,
            },
            address,
            branch,
            ctx,
            host,
        ),
        ExpandedNode::Row { .. } => mount_row(node, address, branch, ctx, host),
        ExpandedNode::Column { .. } => mount_column(node, address, branch, ctx, host),
        ExpandedNode::Popover {
            path,
            open,
            at,
            align,
            anchor,
            content,
        } => mount_popover(
            PopoverNode {
                path: *path,
                open,
                at: *at,
                align: *align,
                anchor,
                content,
                size: effective_size(node, ctx.skin, snapshot),
            },
            address,
            branch,
            ctx,
            host,
        ),
        ExpandedNode::Pressable { path, child, .. } => {
            let child = expanded(child, &child_address(address, 0), branch, ctx, host);
            host.pressable(*path, child, effective_size(node, ctx.skin, snapshot))
        }
        ExpandedNode::Scroll { id, child, .. } => {
            let child = expanded(child, &child_address(address, 0), branch, ctx, host);
            host.scroll(*id, child, effective_size(node, ctx.skin, snapshot))
        }
        ExpandedNode::Slot { children, .. } => {
            let mounted = expanded_children(children, address, branch, snapshot, ctx, host);
            host.slot(mounted, effective_size(node, ctx.skin, snapshot))
        }
        ExpandedNode::Stage { children, .. } => {
            let mounted = expanded_children(children, address, branch, snapshot, ctx, host);
            host.stage(mounted, effective_size(node, ctx.skin, snapshot))
        }
        ExpandedNode::Object {
            pose,
            to,
            phase,
            motion,
            child,
        } => {
            let track = Track {
                from: *pose,
                to: to.as_ref(),
                phase: phase.as_ref(),
                motion: motion.as_ref(),
            };
            mount_object(track, child, address, branch, ctx, host)
        }
        ExpandedNode::Control {
            path, spec, read, ..
        } => host.control(
            *path,
            spec,
            read.as_ref(),
            branch.input_owner,
            effective_size(node, ctx.skin, snapshot),
            branch.transform,
        ),
    }
}

fn mount_popover<H>(
    node: PopoverNode<'_>,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let anchor = expanded(node.anchor, &child_address(address, 0), branch, ctx, host);
    let content = child_address(address, 1);
    host.popover(
        Popover {
            path: node.path,
            at: node.at,
            align: node.align,
            open: ctx.flag(Some(node.open)),
            flag: node.open,
            size: node.size,
        },
        anchor,
        &mut |host| expanded(node.content, &content, branch, ctx, host),
    )
}

fn expanded_children<H>(
    children: &[ExpandedNode],
    address: &[usize],
    branch: Branch,
    snapshot: &dyn Snapshot,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> Vec<H::Output>
where
    H: Host,
{
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| !is_hidden(*child, snapshot))
        .map(|(index, child)| expanded(child, &child_address(address, index), branch, ctx, host))
        .collect()
}

fn mount_group<H>(
    group: Group<'_>,
    children: &[ExpandedNode],
    address: &[usize],
    branch: Branch,
    snapshot: &dyn Snapshot,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let children =
        expanded_group_children(children, group.axis, address, branch, snapshot, ctx, host);
    host.group(group, children)
}

fn expanded_group_children<H>(
    children: &[ExpandedNode],
    axis: Axis,
    address: &[usize],
    branch: Branch,
    snapshot: &dyn Snapshot,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> Vec<GroupMount<H::Output>>
where
    H: Host,
{
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| !is_hidden(*child, snapshot))
        .map(|(index, child)| GroupMount {
            band: band_of(child),
            minimum: main_minimum(child, axis, ctx.skin, snapshot),
            output: expanded(child, &child_address(address, index), branch, ctx, host),
        })
        .collect()
}

/// The band of room one child of a flow stands in.
fn band_of(node: &ExpandedNode) -> Band {
    match node {
        ExpandedNode::Reveal { from, until, .. } => Band::new(*from, *until),
        _ => Band::ALWAYS,
    }
}

/// Where an object starts, where it ends, and what carries it between them.
#[derive(Clone, Copy)]
struct Track<'a> {
    from: Pose,
    to: Option<&'a Pose>,
    phase: Option<&'a Binding>,
    motion: Option<&'a Motion<Binding>>,
}

impl Track<'_> {
    /// The pose to draw this object at, this frame.
    ///
    /// A phase an endpoint answers moves the object between one frame and the
    /// next; a motion works the same scalar out from the seconds its clock
    /// hands over. An object with no track, or one nobody drives, sits at the
    /// pose the document wrote down and stays there.
    fn resolve(self, ctx: Ctx<'_, '_>) -> Pose {
        // Validation refuses an object carrying both, so nothing is chosen
        // between here; one that somehow held both would sit still.
        let along = match (self.phase, self.motion) {
            (Some(phase), None) => scalar(ctx, phase),
            (None, Some(motion)) => scalar(ctx, &motion.clock).map(|at| motion.phase_at(at)),
            (None, None) | (Some(_), Some(_)) => None,
        };
        match (self.to, along) {
            (Some(to), Some(along)) => self.from.between(to, along),
            _ => self.from,
        }
    }
}

/// One scalar an endpoint answers with, or nothing when it answers otherwise.
fn scalar(ctx: Ctx<'_, '_>, binding: &Binding) -> Option<f32> {
    match ctx.read(binding)? {
        ReadValue::Scalar(value) => Some(value.as_()),
        _ => None,
    }
}

/// Composes an object's pose onto whatever its subtree draws.
///
/// The object mounts nothing of its own: the child goes straight to the host,
/// carrying an offset the host applies to the picture and to nothing else.
fn mount_object<H>(
    track: Track<'_>,
    child: &ExpandedNode,
    address: &[usize],
    branch: Branch,
    ctx: Ctx<'_, '_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let here = track.resolve(ctx);
    expanded(
        child,
        &child_address(address, 0),
        Branch {
            transform: here.matrix().then(branch.transform),
            ..branch
        },
        ctx,
        host,
    )
}

fn child_address(parent: &[usize], index: usize) -> Vec<usize> {
    let mut address = Vec::with_capacity(parent.len() + 1);
    address.extend_from_slice(parent);
    address.push(index);
    address
}

fn main_minimum(
    node: &ExpandedNode,
    axis: Axis,
    skin: &SkinDoc,
    snapshot: &dyn Snapshot,
) -> Option<f32> {
    let size = effective_size(node, skin, snapshot)?;
    let dim = match axis {
        Axis::Horizontal => size.w,
        Axis::Vertical => size.h,
    };
    match dim {
        Dim::Range { min, .. } => Some(min),
        _ => None,
    }
}

fn hosts_engine(ui: &CompiledUi, owner: InternId, address: &[usize]) -> bool {
    HOSTED_MODULES
        .iter()
        .any(|module| ui.includes_module(owner, address, module))
}

fn dragged_label(ctx: Ctx<'_, '_>) -> Option<String> {
    let binding = ctx.ui.dragged.as_ref()?;
    match ctx.read(binding)? {
        ReadValue::Text(label) if !label.is_empty() => Some(label.to_owned()),
        _ => None,
    }
}
