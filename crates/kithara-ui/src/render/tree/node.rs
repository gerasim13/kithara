use iced::{
    Alignment, Element, Length, Size,
    widget::{Space, Stack, container, mouse_area},
};
use num_traits::cast::AsPrimitive;

use super::{
    control::render_control,
    flex::Flex,
    geometry::{
        Rendered, active_tone, apply_size, bordered, content_size, effective_size, filled,
        frame_tone, padding,
    },
    host,
    read::{read_flag, resolve},
    size::{node_size, visible_children},
};
use crate::{
    compile::{CompiledNode, CompiledUi},
    expand::{ExpandedNode, SurfaceSpec},
    ids::InternId,
    layout::{Axis, FrameSides},
    module::ChromeStyle,
    render::{
        ControlAction, DragPhase, InputOwner, ReadValue, Reads, Skin, UiEvent, control_event,
    },
    size::{Dim, Hidden, is_hidden},
    skin::ColorRole,
    widgets::{DropZone, ModuleChrome, Widget, anchored::Anchored, wheel::WheelSurface},
};

pub(super) fn render_compiled<'a>(
    node: &CompiledNode,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    let hidden: Hidden<'_> = &|block| read_flag(Some(&block.hidden), reads, ui);
    match node {
        CompiledNode::Optional { child, .. } => render_compiled(child, ui, reads, skin),
        CompiledNode::Split { axis, children, .. } => match axis {
            Axis::Horizontal => container(
                Flex::row_sized(visible_children(children, hidden).map(|(weight, child)| {
                    (
                        render_compiled(child, ui, reads, skin),
                        Size::new(
                            split_length(node_size(child, skin.document(), hidden).w, weight, skin),
                            Length::Fill,
                        ),
                        None,
                    )
                }))
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            Axis::Vertical => container(
                Flex::column_sized(visible_children(children, hidden).map(|(weight, child)| {
                    (
                        render_compiled(child, ui, reads, skin),
                        Size::new(
                            Length::Fill,
                            split_length(node_size(child, skin.document(), hidden).h, weight, skin),
                        ),
                        None,
                    )
                }))
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        },
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
                    reads.get(ui.resolve(*collapsed)),
                    Some(ReadValue::Bool(true))
                );
            let footer = footer
                .as_ref()
                .and_then(|binding| resolve(reads, binding, ui))
                .and_then(|value| match value {
                    ReadValue::Text(text) => Some(text.to_owned()),
                    _ => None,
                });
            let content: Element<'a, UiEvent> = if collapsed {
                Space::new().into()
            } else if hosted_module(ui.resolve(*module)) {
                let child = render_engine_node(root, &[], *instance, ui, reads, skin);
                host::host(child, root, ui, reads, skin)
            } else {
                render_node(
                    root,
                    &[],
                    NodeContext {
                        owner: *instance,
                        ui,
                        reads,
                        skin,
                        input_owner: InputOwner::Leaf,
                    },
                )
            };
            ModuleChrome::builder()
                .content(content)
                .maybe_title(title.map(|id| ui.resolve(id)))
                .maybe_chip(chip.map(|id| ui.resolve(id)))
                .assign(assign.iter().map(|id| ui.resolve(*id)).collect())
                .style(*chrome)
                .frame(*frame)
                .corners(*corners)
                .maybe_footer(footer)
                .maybe_drop(drop.as_ref().map(|drop| {
                    module_drop_zone(
                        ui.resolve(*instance),
                        read_flag(Some(&drop.read), reads, ui),
                    )
                }))
                .on_toggle(UiEvent::ToggleModule(ui.resolve(*module).to_owned()))
                .collapsed(collapsed)
                .skin(skin)
                .build()
                .view()
        }
    }
}

fn module_drop_zone(instance: &str, active: bool) -> DropZone<UiEvent> {
    let crossing = |over| {
        control_event(
            &format!("{instance}/drop"),
            ControlAction::Drag(DragPhase::Over(over)),
        )
    };
    DropZone::new(crossing(true), crossing(false), active)
}

fn wheeled<'a>(
    element: Element<'a, UiEvent>,
    surface: Option<&SurfaceSpec>,
    size: (Length, Length),
    ui: &'a CompiledUi,
) -> Element<'a, UiEvent> {
    let Some(surface) = surface else {
        return element;
    };
    let wheel = WheelSurface::builder()
        .path(ui.resolve(surface.path))
        .build()
        .view();
    Stack::with_children([element, wheel])
        .width(size.0)
        .height(size.1)
        .into()
}

#[derive(Clone, Copy)]
struct NodeContext<'a, 'reads> {
    owner: InternId,
    ui: &'a CompiledUi,
    reads: &'reads dyn Reads,
    skin: &'a Skin,
    input_owner: InputOwner,
}

fn render_node<'a>(
    node: &ExpandedNode,
    address: &[usize],
    context: NodeContext<'a, '_>,
) -> Element<'a, UiEvent> {
    let NodeContext {
        owner,
        ui,
        reads,
        skin,
        input_owner,
    } = context;
    if matches!(input_owner, InputOwner::Leaf) && hosts_engine(ui, owner, address) {
        let child = render_engine_node(node, address, owner, ui, reads, skin);
        return host::host(child, node, ui, reads, skin);
    }

    let size = content_size(node, skin);
    let hidden: Hidden<'_> = &|block| read_flag(Some(&block.hidden), reads, ui);
    let rendered = match node {
        ExpandedNode::Optional { child, .. } => {
            return render_child(child, address, 0, context);
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
        } => {
            let active = read_flag(active.as_ref(), reads, ui);
            render_group(
                Group {
                    axis: Axis::Horizontal,
                    children,
                    gap: *gap,
                    pad: *pad,
                    pad_x: *pad_x,
                    pad_y: *pad_y,
                    frame: *frame,
                    background: active_tone(*background, *active_background, active),
                    background_alpha: *background_alpha,
                    frame_tone: frame_tone(*frame_color, *active_frame_color, active, skin),
                    surface: surface.as_ref(),
                },
                address,
                context,
                size,
                hidden,
            )
        }
        ExpandedNode::Column {
            children,
            gap,
            pad,
            pad_x,
            pad_y,
            frame,
            background,
            background_alpha,
            surface,
            ..
        } => render_group(
            Group {
                axis: Axis::Vertical,
                children,
                gap: *gap,
                pad: *pad,
                pad_x: *pad_x,
                pad_y: *pad_y,
                frame: *frame,
                background: *background,
                background_alpha: *background_alpha,
                frame_tone: frame_tone(None, None, false, skin),
                surface: surface.as_ref(),
            },
            address,
            context,
            size,
            hidden,
        ),
        ExpandedNode::Popover {
            path,
            open,
            at,
            anchor,
            content,
        } => {
            let open = read_flag(Some(open), reads, ui);
            let content: Element<'a, UiEvent> = if open {
                render_child(content, address, 1, context)
            } else {
                Space::new().into()
            };
            Rendered::leading(
                Anchored::new(
                    render_child(anchor, address, 0, context),
                    content,
                    open,
                    *at,
                    control_event(ui.resolve(*path), ControlAction::Activate),
                    skin,
                )
                .into(),
            )
        }
        ExpandedNode::Pressable { path, child, .. } => {
            let path = ui.resolve(*path);
            Rendered::leading(
                mouse_area(render_child(child, address, 0, context))
                    .on_press(control_event(path, ControlAction::Activate))
                    .on_right_press(control_event(path, ControlAction::SecondaryActivate))
                    .into(),
            )
        }
        ExpandedNode::Slot { children, .. } => render_slot(children, address, context, hidden),
        ExpandedNode::Control {
            path, spec, read, ..
        } => render_control(*path, spec, read.as_ref(), ui, reads, skin, input_owner),
    };
    apply_size(rendered, effective_size(node, skin))
}

pub(super) fn render_engine_node<'a>(
    node: &ExpandedNode,
    address: &[usize],
    owner: InternId,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    render_node(
        node,
        address,
        NodeContext {
            owner,
            ui,
            reads,
            skin,
            input_owner: InputOwner::Engine,
        },
    )
}

#[derive(Clone, Copy)]
struct Group<'a> {
    axis: Axis,
    children: &'a [ExpandedNode],
    gap: Option<f32>,
    pad: Option<f32>,
    pad_x: Option<f32>,
    pad_y: Option<f32>,
    frame: Option<FrameSides>,
    background: Option<ColorRole>,
    background_alpha: Option<f32>,
    frame_tone: (ColorRole, f32),
    surface: Option<&'a SurfaceSpec>,
}

fn render_group<'a>(
    group: Group<'_>,
    address: &[usize],
    context: NodeContext<'a, '_>,
    size: (Length, Length),
    hidden: Hidden<'_>,
) -> Rendered<'a> {
    let Group {
        axis,
        children,
        gap,
        pad,
        pad_x,
        pad_y,
        frame,
        background,
        background_alpha,
        frame_tone,
        surface,
    } = group;
    let spacing = gap.unwrap_or(context.skin.layout.grid_gap);
    let flex = match axis {
        Axis::Horizontal => Flex::row(children.iter().enumerate().filter_map(|(index, child)| {
            if is_hidden(child, hidden) {
                return None;
            }
            Some((
                render_child(child, address, index, context),
                main_minimum(child, axis, context.skin),
            ))
        }))
        .spacing(spacing)
        .align(Alignment::Center)
        .width(size.0)
        .height(size.1),
        Axis::Vertical => Flex::column(children.iter().enumerate().filter_map(|(index, child)| {
            if is_hidden(child, hidden) {
                return None;
            }
            Some((
                render_child(child, address, index, context),
                main_minimum(child, axis, context.skin),
            ))
        }))
        .spacing(spacing)
        .align(Alignment::Start)
        .width(size.0),
    };
    Rendered::leading(wheeled(
        bordered(
            filled(
                container(flex)
                    .padding(padding(pad, pad_x, pad_y, context.skin))
                    .width(size.0)
                    .height(size.1),
                background,
                background_alpha,
                context.skin,
            ),
            frame,
            frame_tone,
            size,
            context.skin,
        ),
        surface,
        size,
        context.ui,
    ))
}

fn render_child<'a>(
    node: &ExpandedNode,
    parent: &[usize],
    index: usize,
    context: NodeContext<'a, '_>,
) -> Element<'a, UiEvent> {
    let address = child_address(parent, index);
    render_node(node, &address, context)
}

fn render_slot<'a>(
    children: &[ExpandedNode],
    address: &[usize],
    context: NodeContext<'a, '_>,
    hidden: Hidden<'_>,
) -> Rendered<'a> {
    Rendered::leading(
        container(
            Flex::column(children.iter().enumerate().filter_map(|(index, child)| {
                if is_hidden(child, hidden) {
                    return None;
                }
                Some((render_child(child, address, index, context), None))
            }))
            .spacing(context.skin.layout.grid_gap)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into(),
    )
}

fn child_address(parent: &[usize], index: usize) -> Vec<usize> {
    let mut address = Vec::with_capacity(parent.len() + 1);
    address.extend_from_slice(parent);
    address.push(index);
    address
}

const HOSTED_MODULES: [&str; 9] = [
    "studio-deck",
    "studio-strip",
    "studio-mixer",
    "studio-mixer-single",
    "studio-overview-row",
    "gallery-knobs",
    "gallery-meters",
    "gallery-toggles",
    "gallery-buttons-tab",
];

fn hosted_module(module: &str) -> bool {
    HOSTED_MODULES.contains(&module)
}

fn hosts_engine(ui: &CompiledUi, owner: InternId, address: &[usize]) -> bool {
    HOSTED_MODULES
        .iter()
        .any(|module| ui.includes_module(owner, address, module))
}

fn main_minimum(node: &ExpandedNode, axis: Axis, skin: &Skin) -> Option<f32> {
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

fn split_length(dim: Dim, weight: f32, skin: &Skin) -> Length {
    match dim {
        Dim::Fixed(value) => Length::Fixed(value),
        _ => Length::FillPortion(fill_portion(weight, skin)),
    }
}

fn fill_portion(weight: f32, skin: &Skin) -> u16 {
    let scaled = (weight * skin.layout.fill_weight_scale)
        .round()
        .max(skin.layout.fill_weight_min)
        .min(f32::from(u16::MAX));
    scaled.as_()
}
