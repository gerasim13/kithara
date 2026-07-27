use std::io;

use anyhow::Result;

use super::{
    cli::{ViewName, VizArgs},
    source,
    view::{DEFAULT_NODE_BUDGET, DiagramModel, ViewKind, ViewRequest, project},
};
use crate::Ctx;

pub(crate) fn run(args: &VizArgs, ctx: &Ctx) -> Result<()> {
    let graph = source::collect(ctx, args.krate.as_deref())?;
    let request = ViewRequest {
        kind: view_kind(args.view),
        module: args.module.clone(),
        node_budget: DEFAULT_NODE_BUDGET,
    };
    let model = project(&graph, &request);
    write_model(&model, io::stdout().lock())
}

fn view_kind(view: ViewName) -> ViewKind {
    match view {
        ViewName::Overview => ViewKind::Overview,
        ViewName::Hierarchy => ViewKind::Hierarchy,
        ViewName::Ownership => ViewKind::Ownership,
    }
}

fn write_model(model: &DiagramModel, mut output: impl io::Write) -> Result<()> {
    writeln!(
        output,
        "architecture view: {:?} ({} visible, {} hidden)",
        model.kind,
        model.nodes.len(),
        model.hidden_nodes
    )?;
    for node in &model.nodes {
        if let Some(location) = &node.location {
            writeln!(
                output,
                "node\t{}\t{:?}\t{:?}\t{}\t{}:{}",
                node.id, node.kind, node.style, node.label, location.path, location.line
            )?;
        } else {
            writeln!(
                output,
                "node\t{}\t{:?}\t{:?}\t{}",
                node.id, node.kind, node.style, node.label
            )?;
        }
    }
    for edge in &model.edges {
        writeln!(
            output,
            "edge\t{}\t{}\t{:?}\t{:?}",
            edge.source, edge.target, edge.kind, edge.style
        )?;
    }
    output.flush()?;
    Ok(())
}
