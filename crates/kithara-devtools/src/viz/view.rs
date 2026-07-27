use std::collections::BTreeSet;

use super::graph::{
    Edge, EdgeKind, EvidenceGraph, MergedCertainty, Node, NodeId, NodeKind, SourceLocation,
};

pub(crate) const DEFAULT_NODE_BUDGET: usize = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewKind {
    Overview,
    Hierarchy,
    Ownership,
}

#[derive(Debug)]
pub(crate) struct ViewRequest {
    pub(crate) kind: ViewKind,
    pub(crate) module: Option<String>,
    pub(crate) node_budget: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvidenceStyle {
    Resolved,
    Candidate,
    Unresolved,
    Conflicting,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DiagramNode {
    pub(crate) id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) label: String,
    pub(crate) location: Option<SourceLocation>,
    pub(crate) style: EvidenceStyle,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DiagramEdge {
    pub(crate) source: NodeId,
    pub(crate) target: NodeId,
    pub(crate) kind: EdgeKind,
    pub(crate) style: EvidenceStyle,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DiagramModel {
    pub(crate) kind: ViewKind,
    pub(crate) nodes: Vec<DiagramNode>,
    pub(crate) edges: Vec<DiagramEdge>,
    pub(crate) hidden_nodes: usize,
}

pub(crate) fn project(graph: &EvidenceGraph, request: &ViewRequest) -> DiagramModel {
    let mut candidates = graph
        .nodes()
        .filter(|node| node_in_view(node, request.kind))
        .filter(|node| module_matches(node, request))
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();

    if request.module.is_some() {
        include_connected_resources(graph, request.kind, &mut candidates);
    }

    let candidate_count = candidates.len();
    let visible = candidates
        .into_iter()
        .take(request.node_budget)
        .collect::<BTreeSet<_>>();
    let nodes = graph
        .nodes()
        .filter(|node| visible.contains(&node.id))
        .map(diagram_node)
        .collect();
    let edges = graph
        .edges()
        .filter(|edge| edge_in_view(edge, request.kind))
        .filter(|edge| visible.contains(&edge.source) && visible.contains(&edge.target))
        .map(diagram_edge)
        .collect();

    DiagramModel {
        kind: request.kind,
        nodes,
        edges,
        hidden_nodes: candidate_count.saturating_sub(visible.len()),
    }
}

fn node_in_view(node: &Node, kind: ViewKind) -> bool {
    match kind {
        ViewKind::Overview => true,
        ViewKind::Hierarchy => matches!(
            node.kind,
            NodeKind::Package | NodeKind::Module | NodeKind::PublicItem
        ),
        ViewKind::Ownership => matches!(node.kind, NodeKind::OwnershipSite | NodeKind::Resource),
    }
}

fn edge_in_view(edge: &Edge, kind: ViewKind) -> bool {
    match kind {
        ViewKind::Overview => true,
        ViewKind::Hierarchy => edge.kind == EdgeKind::Contains,
        ViewKind::Ownership => matches!(
            edge.kind,
            EdgeKind::Constructs | EdgeKind::Clones | EdgeKind::Stores
        ),
    }
}

fn module_matches(node: &Node, request: &ViewRequest) -> bool {
    let Some(module) = request.module.as_deref() else {
        return true;
    };
    node.kind == NodeKind::Package
        || node.id.module == module
        || node
            .id
            .module
            .strip_prefix(module)
            .is_some_and(|suffix| suffix.starts_with("::"))
}

fn include_connected_resources(
    graph: &EvidenceGraph,
    kind: ViewKind,
    candidates: &mut BTreeSet<NodeId>,
) {
    if kind == ViewKind::Hierarchy {
        return;
    }
    for edge in graph.edges().filter(|edge| edge_in_view(edge, kind)) {
        if candidates.contains(&edge.source)
            && graph
                .node(&edge.target)
                .is_some_and(|node| node.kind == NodeKind::Resource)
        {
            candidates.insert(edge.target.clone());
        }
        if candidates.contains(&edge.target)
            && graph
                .node(&edge.source)
                .is_some_and(|node| node.kind == NodeKind::Resource)
        {
            candidates.insert(edge.source.clone());
        }
    }
}

fn diagram_node(node: &Node) -> DiagramNode {
    DiagramNode {
        id: node.id.clone(),
        kind: node.kind,
        label: node.label.clone(),
        location: node.location.clone(),
        style: evidence_style(node.certainty()),
    }
}

fn diagram_edge(edge: &Edge) -> DiagramEdge {
    DiagramEdge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        kind: edge.kind,
        style: evidence_style(edge.certainty()),
    }
}

fn evidence_style(certainty: MergedCertainty) -> EvidenceStyle {
    match certainty {
        MergedCertainty::Resolved => EvidenceStyle::Resolved,
        MergedCertainty::Candidate => EvidenceStyle::Candidate,
        MergedCertainty::Unresolved => EvidenceStyle::Unresolved,
        MergedCertainty::Conflicting => EvidenceStyle::Conflicting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viz::graph::{
        Certainty, Edge, EdgeKind, Evidence, EvidenceClass, EvidenceGraph, Node, NodeId, NodeKind,
    };

    fn evidence(origin: &str) -> Evidence {
        Evidence {
            class: EvidenceClass::Static,
            certainty: Certainty::Resolved,
            origin: origin.to_string(),
        }
    }

    #[test]
    fn projection_is_bounded_and_deterministic() {
        let mut graph = EvidenceGraph::default();
        for label in ["zeta", "alpha", "middle"] {
            graph.merge_node(Node::new(
                NodeId::symbol("demo", "lib", "runtime", label),
                NodeKind::PublicItem,
                label,
                evidence(label),
            ));
        }

        let request = ViewRequest {
            kind: ViewKind::Overview,
            module: None,
            node_budget: 2,
        };
        let model = project(&graph, &request);

        assert_eq!(model.hidden_nodes, 1);
        assert_eq!(
            model
                .nodes
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "middle"]
        );
    }

    #[test]
    fn ownership_view_excludes_hierarchy_only_nodes() {
        let mut graph = EvidenceGraph::default();
        let module = NodeId::module("demo", "lib", "runtime");
        let site = NodeId::site("demo", "lib", "runtime", "arc-new@10");
        let resource = NodeId::resource("demo", "lib", "SharedState");
        graph.merge_node(Node::new(
            module.clone(),
            NodeKind::Module,
            "runtime",
            evidence("module"),
        ));
        graph.merge_node(Node::new(
            site.clone(),
            NodeKind::OwnershipSite,
            "Arc::new(...)",
            evidence("site"),
        ));
        graph.merge_node(Node::new(
            resource.clone(),
            NodeKind::Resource,
            "Arc<SharedState>",
            evidence("resource"),
        ));
        graph.merge_edge(Edge::new(
            site,
            resource,
            EdgeKind::Constructs,
            evidence("constructs"),
        ));

        let model = project(
            &graph,
            &ViewRequest {
                kind: ViewKind::Ownership,
                module: None,
                node_budget: 20,
            },
        );

        assert_eq!(model.nodes.len(), 2);
        assert!(model.nodes.iter().all(|node| node.id != module));
        assert_eq!(model.edges.len(), 1);
    }
}
