use std::collections::BTreeSet;

use crate::{
    compile::{CompiledNode, CompiledUi},
    expand::ExpandedNode,
};

/// Which of `required` the screen answers on nowhere, in the order asked.
pub(crate) fn missing(ui: &CompiledUi, required: &[&str]) -> Vec<String> {
    if required.is_empty() {
        return Vec::new();
    }
    let mut answered = BTreeSet::new();
    collect(&ui.root, ui, &mut answered);
    required
        .iter()
        .filter(|path| !answered.contains(**path))
        .map(|path| (*path).to_owned())
        .collect()
}

fn collect<'u>(node: &CompiledNode, ui: &'u CompiledUi, into: &mut BTreeSet<&'u str>) {
    match node {
        CompiledNode::Split { children, .. } => {
            for cell in children {
                collect(&cell.node, ui, into);
            }
        }
        CompiledNode::Optional { child, .. } => collect(child, ui, into),
        CompiledNode::Adaptive { base, steps, .. } => {
            collect(base, ui, into);
            for (_, branch) in steps {
                collect(branch, ui, into);
            }
        }
        CompiledNode::Module { root, .. } => collect_expanded(root, ui, into),
    }
}

fn collect_expanded<'u>(node: &ExpandedNode, ui: &'u CompiledUi, into: &mut BTreeSet<&'u str>) {
    match node {
        ExpandedNode::Row { children, .. }
        | ExpandedNode::Column { children, .. }
        | ExpandedNode::Slot { children, .. }
        | ExpandedNode::Stage { children, .. } => {
            for child in children {
                collect_expanded(child, ui, into);
            }
        }
        ExpandedNode::Object { child, .. }
        | ExpandedNode::Optional { child, .. }
        | ExpandedNode::Reveal { child, .. }
        | ExpandedNode::Scroll { child, .. } => collect_expanded(child, ui, into),
        ExpandedNode::Adaptive { base, steps, .. } => {
            collect_expanded(base, ui, into);
            for (_, branch) in steps {
                collect_expanded(branch, ui, into);
            }
        }
        ExpandedNode::Popover {
            path,
            anchor,
            content,
            ..
        } => {
            into.insert(ui.resolve(*path));
            collect_expanded(anchor, ui, into);
            collect_expanded(content, ui, into);
        }
        ExpandedNode::Pressable { path, child, .. } => {
            into.insert(ui.resolve(*path));
            collect_expanded(child, ui, into);
        }
        ExpandedNode::Control { path, .. } => {
            into.insert(ui.resolve(*path));
        }
    }
}
