use num_traits::cast::AsPrimitive;

use super::{
    Group, Host, Module, Popover,
    read::{read_flag, resolve},
};
use crate::{
    compile::{CompiledNode, CompiledUi},
    draw::Transform,
    expand::{Binding, ExpandedNode, SurfaceSpec},
    ids::InternId,
    layout::{Axis, FrameSides},
    module::{ChromeStyle, PopoverAlign, PopoverAt, Pose, TextAlign},
    render::{InputOwner, ReadValue, Reads},
    size::{
        Dim, Hidden, SizeSpec, compiled_node_size_with_hidden, effective_size, is_hidden,
        visible_compiled_children,
    },
    skin::{ColorRole, SkinDoc},
};

const HOSTED_MODULES: [&str; 21] = [
    "studio-deck",
    "studio-strip",
    "studio-strip-eq-3-band",
    "studio-strip-eq-4-band",
    "studio-mixer",
    "studio-mixer-single",
    "studio-overview",
    "studio-overview-row",
    "studio-overview-single",
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
pub fn render<H>(
    node: &CompiledNode,
    ui: &CompiledUi,
    reads: &dyn Reads,
    skin: &SkinDoc,
    mut host: H,
) -> H::Output
where
    H: Host,
{
    let walk = Walk { ui, reads, skin };
    let content = compiled(node, walk, &mut host);
    host.window(content, dragged_label(ui, reads), ui.resize_edges)
}

#[cfg(test)]
pub(crate) fn render_engine_subtree<H>(
    node: &ExpandedNode,
    address: &[usize],
    owner: InternId,
    ui: &CompiledUi,
    reads: &dyn Reads,
    skin: &SkinDoc,
    mut host: H,
) -> H::Output
where
    H: Host,
{
    let walk = Walk { ui, reads, skin };
    expanded(
        node,
        address,
        Context {
            owner,
            input_owner: InputOwner::Engine,
            transform: Transform::IDENTITY,
        },
        walk,
        &mut host,
    )
}

fn compiled<H>(node: &CompiledNode, walk: Walk<'_>, host: &mut H) -> H::Output
where
    H: Host,
{
    let hidden: Hidden<'_> = &|block| read_flag(Some(&block.hidden), walk.reads, walk.ui);
    match node {
        CompiledNode::Optional { child, .. } => compiled(child, walk, host),
        CompiledNode::Split { axis, children, .. } => {
            let mut mounted = Vec::with_capacity(children.len());
            for (weight, child) in visible_compiled_children(children, hidden) {
                let size = compiled_node_size_with_hidden(child, walk.skin, hidden);
                let output = compiled(child, walk, host);
                mounted.push((weight, size, output));
            }
            host.split(*axis, mounted)
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
                    walk.reads.get(walk.ui.resolve(*collapsed)),
                    Some(ReadValue::Bool(true))
                );
            let footer = footer
                .as_ref()
                .and_then(|binding| resolve(walk.reads, binding, walk.ui))
                .and_then(|value| match value {
                    ReadValue::Text(text) => Some(text.to_owned()),
                    _ => None,
                });
            let content_hosted = HOSTED_MODULES.contains(&walk.ui.resolve(*module));
            let chrome_hosted = *chrome == ChromeStyle::Full || drop.is_some();
            let content = (!collapsed).then(|| {
                let child = expanded(
                    root,
                    &[],
                    Context {
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
                    walk,
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
struct Context {
    owner: InternId,
    input_owner: InputOwner,
    /// Every enclosing object's pose, composed and resolved for this frame.
    transform: Transform,
}

#[derive(Clone, Copy)]
struct Walk<'a> {
    ui: &'a CompiledUi,
    reads: &'a dyn Reads,
    skin: &'a SkinDoc,
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
    gap: Option<f32>,
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

fn row_group<'a>(node: RowNode<'a>, walk: Walk<'_>) -> Group<'a> {
    let active = read_flag(node.active, walk.reads, walk.ui);
    let padding = node.pad.unwrap_or(walk.skin.layout.grid_pad);
    let background = active
        .then_some(node.active_background)
        .flatten()
        .or(node.background);
    let frame_color = active
        .then_some(node.active_frame_color)
        .flatten()
        .or(node.frame_color)
        .unwrap_or(walk.skin.divider.color);
    Group {
        axis: Axis::Horizontal,
        alignment: TextAlign::Center,
        gap: node.gap.unwrap_or(walk.skin.layout.grid_gap),
        padding_x: node.pad_x.unwrap_or(padding),
        padding_y: node.pad_y.unwrap_or(padding),
        frame: node.frame,
        background,
        background_alpha: node.background_alpha,
        frame_color,
        frame_width: walk.skin.divider.width,
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
    context: Context,
    walk: Walk<'_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    if context.input_owner == InputOwner::Leaf && hosts_engine(walk.ui, context.owner, address) {
        let child = expanded(
            node,
            address,
            Context {
                input_owner: InputOwner::Engine,
                ..context
            },
            walk,
            host,
        );
        return host.hosted(node, child);
    }
    mounted(node, address, context, walk, host)
}

/// How one node becomes host output, once the engine question is settled.
fn mounted<H>(
    node: &ExpandedNode,
    address: &[usize],
    context: Context,
    walk: Walk<'_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let hidden: Hidden<'_> = &|block| read_flag(Some(&block.hidden), walk.reads, walk.ui);
    match node {
        ExpandedNode::Optional { child, .. } => {
            expanded(child, &child_address(address, 0), context, walk, host)
        }
        ExpandedNode::Row {
            children,
            gap,
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
        } => mount_group(
            row_group(
                RowNode {
                    gap: *gap,
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
                    size: effective_size(node, walk.skin),
                },
                walk,
            ),
            children,
            address,
            context,
            hidden,
            walk,
            host,
        ),
        ExpandedNode::Column {
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
        } => mount_group(
            Group {
                axis: Axis::Vertical,
                alignment: *align,
                gap: gap.unwrap_or(walk.skin.layout.grid_gap),
                padding_x: pad_x.unwrap_or(pad.unwrap_or(walk.skin.layout.grid_pad)),
                padding_y: pad_y.unwrap_or(pad.unwrap_or(walk.skin.layout.grid_pad)),
                frame: *frame,
                background: *background,
                background_alpha: *background_alpha,
                frame_color: walk.skin.divider.color,
                frame_width: walk.skin.divider.width,
                surface: surface.as_ref(),
                size: effective_size(node, walk.skin),
            },
            children,
            address,
            context,
            hidden,
            walk,
            host,
        ),
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
                size: effective_size(node, walk.skin),
            },
            address,
            context,
            walk,
            host,
        ),
        ExpandedNode::Pressable { path, child, .. } => {
            let child = expanded(child, &child_address(address, 0), context, walk, host);
            host.pressable(*path, child, effective_size(node, walk.skin))
        }
        ExpandedNode::Scroll { id, child, .. } => {
            let child = expanded(child, &child_address(address, 0), context, walk, host);
            host.scroll(*id, child, effective_size(node, walk.skin))
        }
        ExpandedNode::Slot { children, .. } => {
            let mounted = expanded_children(children, address, context, hidden, walk, host);
            host.slot(mounted, effective_size(node, walk.skin))
        }
        ExpandedNode::Stage { children, .. } => {
            let mounted = expanded_children(children, address, context, hidden, walk, host);
            host.stage(mounted, effective_size(node, walk.skin))
        }
        ExpandedNode::Object {
            pose,
            to,
            phase,
            child,
        } => {
            let track = Track(*pose, to.as_ref(), phase.as_ref());
            mount_object(track, child, address, context, walk, host)
        }
        ExpandedNode::Control {
            path, spec, read, ..
        } => host.control(
            *path,
            spec,
            read.as_ref(),
            context.input_owner,
            effective_size(node, walk.skin),
            context.transform,
        ),
    }
}

fn mount_popover<H>(
    node: PopoverNode<'_>,
    address: &[usize],
    context: Context,
    walk: Walk<'_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let open = read_flag(Some(node.open), walk.reads, walk.ui);
    let anchor = expanded(node.anchor, &child_address(address, 0), context, walk, host);
    let content = open.then(|| {
        expanded(
            node.content,
            &child_address(address, 1),
            context,
            walk,
            host,
        )
    });
    host.popover(
        Popover {
            path: node.path,
            at: node.at,
            align: node.align,
            open,
            size: node.size,
        },
        anchor,
        content,
    )
}

fn expanded_children<H>(
    children: &[ExpandedNode],
    address: &[usize],
    context: Context,
    hidden: Hidden<'_>,
    walk: Walk<'_>,
    host: &mut H,
) -> Vec<H::Output>
where
    H: Host,
{
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| !is_hidden(*child, hidden))
        .map(|(index, child)| expanded(child, &child_address(address, index), context, walk, host))
        .collect()
}

fn mount_group<H>(
    group: Group<'_>,
    children: &[ExpandedNode],
    address: &[usize],
    context: Context,
    hidden: Hidden<'_>,
    walk: Walk<'_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let children =
        expanded_group_children(children, group.axis, address, context, hidden, walk, host);
    host.group(group, children)
}

fn expanded_group_children<H>(
    children: &[ExpandedNode],
    axis: Axis,
    address: &[usize],
    context: Context,
    hidden: Hidden<'_>,
    walk: Walk<'_>,
    host: &mut H,
) -> Vec<(Option<f32>, H::Output)>
where
    H: Host,
{
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| !is_hidden(*child, hidden))
        .map(|(index, child)| {
            let minimum = main_minimum(child, axis, walk.skin);
            let output = expanded(child, &child_address(address, index), context, walk, host);
            (minimum, output)
        })
        .collect()
}

/// Where an object starts, where it ends, and what says how far along it is.
#[derive(Clone, Copy)]
struct Track<'a>(Pose, Option<&'a Pose>, Option<&'a Binding>);

/// Composes an object's pose onto whatever its subtree draws.
///
/// The object mounts nothing of its own: the child goes straight to the host,
/// carrying an offset the host applies to the picture and to nothing else.
fn mount_object<H>(
    track: Track<'_>,
    child: &ExpandedNode,
    address: &[usize],
    context: Context,
    walk: Walk<'_>,
    host: &mut H,
) -> H::Output
where
    H: Host,
{
    let Track(pose, to, phase) = track;
    // A `phase` an endpoint answers moves the object along its track between
    // one frame and the next; an object with no track, or one nobody drives,
    // sits at the pose the document wrote down.
    let along = phase
        .and_then(|binding| resolve(walk.reads, binding, walk.ui))
        .and_then(|value| match value {
            ReadValue::Scalar(along) => Some(along.as_()),
            _ => None,
        });
    let here = match (to, along) {
        (Some(to), Some(along)) => pose.between(to, along),
        _ => pose,
    };
    expanded(
        child,
        &child_address(address, 0),
        Context {
            transform: here.matrix().then(context.transform),
            ..context
        },
        walk,
        host,
    )
}

fn child_address(parent: &[usize], index: usize) -> Vec<usize> {
    let mut address = Vec::with_capacity(parent.len() + 1);
    address.extend_from_slice(parent);
    address.push(index);
    address
}

fn main_minimum(node: &ExpandedNode, axis: Axis, skin: &SkinDoc) -> Option<f32> {
    let size = effective_size(node, skin)?;
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

fn dragged_label(ui: &CompiledUi, reads: &dyn Reads) -> Option<String> {
    let binding = ui.dragged.as_ref()?;
    match resolve(reads, binding, ui)? {
        ReadValue::Text(label) if !label.is_empty() => Some(label.to_owned()),
        _ => None,
    }
}
