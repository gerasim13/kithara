use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{
    filter::ArchitectureFilter,
    graph::{
        Edge, EdgeKind, EvidenceGraph, MergedCertainty, Node, NodeId, NodeKind, SourceLocation,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ViewKind {
    Overview,
    Hierarchy,
    Ownership,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DetailLevel {
    Crates,
    Modules,
    Abstractions,
    Methods,
    Full,
}

impl DetailLevel {
    pub(crate) fn as_u8(self) -> u8 {
        match self {
            Self::Crates => 0,
            Self::Modules => 1,
            Self::Abstractions => 2,
            Self::Methods => 3,
            Self::Full => 4,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ViewRequest {
    pub(crate) kind: ViewKind,
    pub(crate) package: Option<String>,
    pub(crate) module: Option<String>,
    pub(crate) scenario: Option<String>,
    pub(crate) lod: DetailLevel,
    pub(crate) filter: ArchitectureFilter,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceStyle {
    Resolved,
    Conditional,
    Observed,
    Manual,
    Candidate,
    Unresolved,
    Conflicting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct DiagramNode {
    pub(crate) id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) label: String,
    pub(crate) location: Option<SourceLocation>,
    pub(crate) style: EvidenceStyle,
    pub(crate) parent: Option<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct DiagramEdge {
    pub(crate) source: NodeId,
    pub(crate) target: NodeId,
    pub(crate) kind: EdgeKind,
    pub(crate) style: EvidenceStyle,
    pub(crate) count: usize,
    pub(crate) details: Vec<String>,
    pub(crate) origins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct DiagramGroup {
    pub(crate) node: NodeId,
    pub(crate) members: Vec<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct DiagramModel {
    pub(crate) kind: ViewKind,
    pub(crate) lod: DetailLevel,
    pub(crate) nodes: Vec<DiagramNode>,
    pub(crate) edges: Vec<DiagramEdge>,
    pub(crate) groups: Vec<DiagramGroup>,
    pub(crate) hidden_nodes: usize,
}

pub(crate) fn project(graph: &EvidenceGraph, request: &ViewRequest) -> DiagramModel {
    if request.lod == DetailLevel::Crates {
        return project_crates(graph, request);
    }

    let parents = containment_parents(graph);
    let boundary = boundary_functions(graph, &parents);
    let eligible = graph
        .nodes()
        .filter(|node| {
            node_visible_at_lod(node, request.lod, &boundary)
                && (node_in_view(node, request.kind)
                    || matches!(node.kind, NodeKind::Package | NodeKind::Module))
        })
        .filter(|node| scenario_matches(node, request))
        .filter(|node| !request.filter.excludes(&node.id))
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    let mut candidates = eligible
        .iter()
        .filter(|id| {
            graph
                .node(id)
                .is_some_and(|node| module_matches(node, request))
        })
        .filter(|id| {
            request
                .package
                .as_ref()
                .is_none_or(|package| id.package == *package)
        })
        .cloned()
        .collect::<BTreeSet<_>>();

    if request.package.is_some() {
        include_immediate_neighbors(graph, request.kind, &eligible, &mut candidates);
    }

    if request.module.is_some() {
        include_connected_resources(graph, request.kind, &mut candidates);
    }
    include_visible_ancestors(&parents, &eligible, &mut candidates);

    let visible = candidates;
    let nodes = graph
        .nodes()
        .filter(|node| visible.contains(&node.id))
        .map(|node| diagram_node(node, visible_parent(&node.id, &visible, &parents)))
        .collect();
    let edges = lifted_edges(graph, request, &visible, &parents);

    let mut model = DiagramModel {
        kind: request.kind,
        lod: request.lod,
        nodes,
        edges,
        groups: Vec::new(),
        hidden_nodes: 0,
    };
    if request.lod == DetailLevel::Full {
        super::cycle::collapse(&mut model);
    }
    model
}

fn include_visible_ancestors(
    parents: &BTreeMap<NodeId, NodeId>,
    eligible: &BTreeSet<NodeId>,
    candidates: &mut BTreeSet<NodeId>,
) {
    let selected = candidates.clone();
    for id in selected {
        let mut current = &id;
        let mut seen = BTreeSet::new();
        while let Some(parent) = parents.get(current) {
            if !seen.insert(parent.clone()) {
                break;
            }
            if eligible.contains(parent) {
                candidates.insert(parent.clone());
            }
            current = parent;
        }
    }
}

fn lifted_edges(
    graph: &EvidenceGraph,
    request: &ViewRequest,
    visible: &BTreeSet<NodeId>,
    parents: &BTreeMap<NodeId, NodeId>,
) -> Vec<DiagramEdge> {
    let mut lifted = BTreeMap::new();

    for edge in graph
        .edges()
        .filter(|edge| edge_in_view(edge, request.kind))
        .filter(|edge| package_edge_matches(edge, request))
        .filter(|edge| module_edge_matches(edge, request))
        .filter(|edge| {
            !request.filter.excludes(&edge.source) && !request.filter.excludes(&edge.target)
        })
    {
        let Some(source) = nearest_visible(&edge.source, visible, parents) else {
            continue;
        };
        let Some(target) = nearest_visible(&edge.target, visible, parents) else {
            continue;
        };
        if source == target {
            continue;
        }
        let style = evidence_style(
            edge.certainty(),
            edge.evidence.iter().map(|evidence| evidence.class),
        );
        let count = edge
            .evidence
            .iter()
            .map(|evidence| evidence.origin.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            .max(1);
        let detail = edge_detail(graph, edge);
        let origins = edge
            .evidence
            .iter()
            .map(|evidence| evidence.origin.clone())
            .collect::<Vec<_>>();
        lifted
            .entry((source.clone(), target.clone(), edge.kind))
            .and_modify(|current: &mut DiagramEdge| {
                current.style = merge_style(current.style, style);
                current.count += count;
                if !current.details.contains(&detail) {
                    current.details.push(detail.clone());
                    current.details.sort();
                }
                current.origins.extend(origins.iter().cloned());
                current.origins.sort();
                current.origins.dedup();
            })
            .or_insert_with(|| DiagramEdge {
                source,
                target,
                kind: edge.kind,
                style,
                count,
                details: vec![detail],
                origins,
            });
    }
    lifted.into_values().collect()
}

fn containment_parents(graph: &EvidenceGraph) -> BTreeMap<NodeId, NodeId> {
    graph
        .edges()
        .filter(|edge| edge.kind == EdgeKind::Contains)
        .map(|edge| (edge.target.clone(), edge.source.clone()))
        .collect()
}

fn boundary_functions(
    graph: &EvidenceGraph,
    parents: &BTreeMap<NodeId, NodeId>,
) -> BTreeSet<NodeId> {
    let mut boundary = BTreeSet::new();
    for edge in graph.edges().filter(|edge| {
        !matches!(
            edge.kind,
            EdgeKind::Contains | EdgeKind::DependsOn | EdgeKind::Implements
        )
    }) {
        let source_owner = abstraction_ancestor(graph, &edge.source, parents);
        let target_owner = abstraction_ancestor(graph, &edge.target, parents);
        if source_owner == target_owner && source_owner.is_some() {
            continue;
        }
        for id in [&edge.source, &edge.target] {
            if let Some(function) = function_ancestor(graph, id, parents) {
                boundary.insert(function);
            }
        }
    }
    boundary
}

fn function_ancestor(
    graph: &EvidenceGraph,
    id: &NodeId,
    parents: &BTreeMap<NodeId, NodeId>,
) -> Option<NodeId> {
    let mut current = id;
    let mut seen = BTreeSet::new();
    loop {
        if graph.node(current).is_some_and(|node| {
            matches!(
                node.kind,
                NodeKind::Function | NodeKind::PublicFunction | NodeKind::Constructor
            )
        }) {
            return Some(current.clone());
        }
        if !seen.insert(current.clone()) {
            return None;
        }
        current = parents.get(current)?;
    }
}

fn abstraction_ancestor(
    graph: &EvidenceGraph,
    id: &NodeId,
    parents: &BTreeMap<NodeId, NodeId>,
) -> Option<NodeId> {
    let mut current = id;
    let mut seen = BTreeSet::new();
    loop {
        if graph.node(current).is_some_and(|node| {
            matches!(
                node.kind,
                NodeKind::ConcreteType | NodeKind::Trait | NodeKind::ModuleFunctions
            )
        }) {
            return Some(current.clone());
        }
        if !seen.insert(current.clone()) {
            return None;
        }
        current = parents.get(current)?;
    }
}

fn edge_detail(graph: &EvidenceGraph, edge: &Edge) -> String {
    let source = graph
        .node(&edge.source)
        .map_or_else(|| edge.source.symbol.as_str(), |node| node.label.as_str());
    let target = graph
        .node(&edge.target)
        .map_or_else(|| edge.target.symbol.as_str(), |node| node.label.as_str());
    format!("{source} -> {target}")
}

fn nearest_visible(
    id: &NodeId,
    visible: &BTreeSet<NodeId>,
    parents: &BTreeMap<NodeId, NodeId>,
) -> Option<NodeId> {
    let mut current = id;
    let mut seen = BTreeSet::new();
    loop {
        if visible.contains(current) {
            return Some(current.clone());
        }
        if !seen.insert(current.clone()) {
            return None;
        }
        current = parents.get(current)?;
    }
}

fn visible_parent(
    id: &NodeId,
    visible: &BTreeSet<NodeId>,
    parents: &BTreeMap<NodeId, NodeId>,
) -> Option<NodeId> {
    let parent = parents.get(id)?;
    nearest_visible(parent, visible, parents)
}

fn module_edge_matches(edge: &Edge, request: &ViewRequest) -> bool {
    let Some(module) = request.module.as_deref() else {
        return true;
    };
    edge.kind == EdgeKind::DependsOn
        || id_module_matches(&edge.source, module)
        || id_module_matches(&edge.target, module)
}

fn id_module_matches(id: &NodeId, module: &str) -> bool {
    id.module == module
        || id
            .module
            .strip_prefix(module)
            .is_some_and(|suffix| suffix.starts_with("::"))
}

fn project_crates(graph: &EvidenceGraph, request: &ViewRequest) -> DiagramModel {
    let mut visible = graph
        .nodes()
        .filter(|node| node.kind == NodeKind::Package)
        .filter(|node| !request.filter.excludes(&node.id))
        .filter(|node| {
            request
                .package
                .as_ref()
                .is_none_or(|package| node.id.package == *package)
        })
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();

    if request.package.is_some() {
        let selected = visible.clone();
        for edge in graph
            .edges()
            .filter(|edge| edge.kind == EdgeKind::DependsOn)
            .filter(|edge| {
                !request.filter.excludes(&edge.source) && !request.filter.excludes(&edge.target)
            })
        {
            if selected.contains(&edge.source) {
                visible.insert(edge.target.clone());
            }
            if selected.contains(&edge.target) {
                visible.insert(edge.source.clone());
            }
        }
    }

    DiagramModel {
        kind: request.kind,
        lod: request.lod,
        nodes: graph
            .nodes()
            .filter(|node| visible.contains(&node.id))
            .map(|node| diagram_node(node, None))
            .collect(),
        edges: graph
            .edges()
            .filter(|edge| edge.kind == EdgeKind::DependsOn)
            .filter(|edge| package_edge_matches(edge, request))
            .filter(|edge| {
                !request.filter.excludes(&edge.source) && !request.filter.excludes(&edge.target)
            })
            .filter(|edge| visible.contains(&edge.source) && visible.contains(&edge.target))
            .map(diagram_edge)
            .collect(),
        groups: Vec::new(),
        hidden_nodes: 0,
    }
}

fn package_edge_matches(edge: &Edge, request: &ViewRequest) -> bool {
    request
        .package
        .as_ref()
        .is_none_or(|package| edge.source.package == *package || edge.target.package == *package)
}

fn include_immediate_neighbors(
    graph: &EvidenceGraph,
    kind: ViewKind,
    eligible: &BTreeSet<NodeId>,
    candidates: &mut BTreeSet<NodeId>,
) {
    let selected = candidates.clone();
    for edge in graph.edges().filter(|edge| edge_in_view(edge, kind)) {
        if selected.contains(&edge.source) && eligible.contains(&edge.target) {
            candidates.insert(edge.target.clone());
        }
        if selected.contains(&edge.target) && eligible.contains(&edge.source) {
            candidates.insert(edge.source.clone());
        }
    }
}

fn node_in_view(node: &Node, kind: ViewKind) -> bool {
    if matches!(
        node.kind,
        NodeKind::ConcreteType | NodeKind::Trait | NodeKind::ModuleFunctions
    ) {
        return true;
    }
    match kind {
        ViewKind::Overview => matches!(
            node.kind,
            NodeKind::Function
                | NodeKind::PublicFunction
                | NodeKind::Constructor
                | NodeKind::TraitMethod
                | NodeKind::OwnershipSite
                | NodeKind::Resource
                | NodeKind::Task
                | NodeKind::Scenario
                | NodeKind::RuntimeEvent
                | NodeKind::CycleGroup
        ),
        ViewKind::Hierarchy => matches!(
            node.kind,
            NodeKind::Package
                | NodeKind::Module
                | NodeKind::PublicItem
                | NodeKind::PublicFunction
                | NodeKind::Constructor
                | NodeKind::TraitMethod
        ),
        ViewKind::Ownership => matches!(node.kind, NodeKind::OwnershipSite | NodeKind::Resource),
    }
}

fn node_visible_at_lod(node: &Node, lod: DetailLevel, boundary: &BTreeSet<NodeId>) -> bool {
    match lod {
        DetailLevel::Crates => node.kind == NodeKind::Package,
        DetailLevel::Modules => matches!(node.kind, NodeKind::Package | NodeKind::Module),
        DetailLevel::Abstractions => matches!(
            node.kind,
            NodeKind::Package
                | NodeKind::Module
                | NodeKind::ConcreteType
                | NodeKind::Trait
                | NodeKind::ModuleFunctions
        ),
        DetailLevel::Methods => {
            matches!(
                node.kind,
                NodeKind::Package
                    | NodeKind::Module
                    | NodeKind::ConcreteType
                    | NodeKind::Trait
                    | NodeKind::ModuleFunctions
                    | NodeKind::Constructor
                    | NodeKind::TraitMethod
                    | NodeKind::PublicFunction
                    | NodeKind::Resource
                    | NodeKind::Task
            ) || boundary.contains(&node.id)
        }
        DetailLevel::Full => true,
    }
}

fn edge_in_view(edge: &Edge, kind: ViewKind) -> bool {
    match kind {
        ViewKind::Overview => edge.kind != EdgeKind::Contains,
        ViewKind::Hierarchy => matches!(edge.kind, EdgeKind::Contains | EdgeKind::DependsOn),
        ViewKind::Ownership => matches!(
            edge.kind,
            EdgeKind::Constructs
                | EdgeKind::Clones
                | EdgeKind::Stores
                | EdgeKind::Transfers
                | EdgeKind::Sends
                | EdgeKind::Drops
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

fn scenario_matches(node: &Node, request: &ViewRequest) -> bool {
    let Some(scenario) = request.scenario.as_deref() else {
        return true;
    };
    let prefix = format!("trace:{scenario}");
    node.evidence
        .iter()
        .any(|evidence| evidence.origin.starts_with(&prefix))
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

fn diagram_node(node: &Node, parent: Option<NodeId>) -> DiagramNode {
    DiagramNode {
        id: node.id.clone(),
        kind: node.kind,
        label: node.label.clone(),
        location: node.location.clone(),
        style: evidence_style(
            node.certainty(),
            node.evidence.iter().map(|evidence| evidence.class),
        ),
        parent,
    }
}

fn diagram_edge(edge: &Edge) -> DiagramEdge {
    DiagramEdge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        kind: edge.kind,
        style: evidence_style(
            edge.certainty(),
            edge.evidence.iter().map(|evidence| evidence.class),
        ),
        count: edge
            .evidence
            .iter()
            .map(|evidence| evidence.origin.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            .max(1),
        details: vec![format!("{} -> {}", edge.source.symbol, edge.target.symbol)],
        origins: edge
            .evidence
            .iter()
            .map(|evidence| evidence.origin.clone())
            .collect(),
    }
}

fn evidence_style(
    certainty: MergedCertainty,
    classes: impl Iterator<Item = super::graph::EvidenceClass>,
) -> EvidenceStyle {
    let classes = classes.collect::<BTreeSet<_>>();
    if classes.contains(&super::graph::EvidenceClass::Manual) {
        return EvidenceStyle::Manual;
    }
    if classes.contains(&super::graph::EvidenceClass::Observed) {
        return EvidenceStyle::Observed;
    }
    if classes.contains(&super::graph::EvidenceClass::Conditional) {
        return EvidenceStyle::Conditional;
    }
    match certainty {
        MergedCertainty::Resolved => EvidenceStyle::Resolved,
        MergedCertainty::Candidate => EvidenceStyle::Candidate,
        MergedCertainty::Unresolved => EvidenceStyle::Unresolved,
        MergedCertainty::Conflicting => EvidenceStyle::Conflicting,
    }
}

fn merge_style(left: EvidenceStyle, right: EvidenceStyle) -> EvidenceStyle {
    use EvidenceStyle::{
        Candidate, Conditional, Conflicting, Manual, Observed, Resolved, Unresolved,
    };

    match (left, right) {
        (Conflicting, _) | (_, Conflicting) => Conflicting,
        (Unresolved, _) | (_, Unresolved) => Unresolved,
        (Manual, _) | (_, Manual) => Manual,
        (Observed, _) | (_, Observed) => Observed,
        (Candidate, _) | (_, Candidate) => Candidate,
        (Conditional, _) | (_, Conditional) => Conditional,
        (Resolved, Resolved) => Resolved,
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
    fn projection_is_unbounded_and_deterministic() {
        let mut graph = EvidenceGraph::default();
        for label in ["zeta", "alpha", "middle"] {
            graph.merge_node(Node::new(
                NodeId::symbol("demo", "lib", "runtime", label),
                NodeKind::Function,
                label,
                evidence(label),
            ));
        }

        let request = ViewRequest {
            kind: ViewKind::Overview,
            package: None,
            module: None,
            scenario: None,
            lod: DetailLevel::Full,
            filter: ArchitectureFilter::default(),
        };
        let model = project(&graph, &request);

        assert_eq!(model.hidden_nodes, 0);
        assert_eq!(
            model
                .nodes
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "middle", "zeta"]
        );
    }

    #[test]
    fn ownership_view_keeps_module_contour_around_ownership_nodes() {
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
                package: None,
                module: None,
                scenario: None,
                lod: DetailLevel::Full,
                filter: ArchitectureFilter::default(),
            },
        );

        assert_eq!(model.nodes.len(), 3);
        assert!(model.nodes.iter().any(|node| node.id == module));
        assert_eq!(model.edges.len(), 1);
    }

    #[test]
    fn recursive_call_component_collapses_into_one_group() {
        let mut graph = EvidenceGraph::default();
        let first = NodeId::symbol("demo", "lib", "runtime", "first");
        let second = NodeId::symbol("demo", "lib", "runtime", "second");
        for (id, label) in [(&first, "first"), (&second, "second")] {
            graph.merge_node(Node::new(
                id.clone(),
                NodeKind::Function,
                label,
                evidence(label),
            ));
        }
        graph.merge_edge(Edge::new(
            first.clone(),
            second.clone(),
            EdgeKind::Calls,
            evidence("first-second"),
        ));
        graph.merge_edge(Edge::new(
            second,
            first,
            EdgeKind::Calls,
            evidence("second-first"),
        ));

        let model = project(
            &graph,
            &ViewRequest {
                kind: ViewKind::Overview,
                package: None,
                module: None,
                scenario: None,
                lod: DetailLevel::Full,
                filter: ArchitectureFilter::default(),
            },
        );

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.nodes.len(), 1);
        assert_eq!(model.nodes[0].kind, NodeKind::CycleGroup);
        assert!(model.edges.is_empty());
    }

    #[test]
    fn package_projection_includes_only_immediate_cross_package_neighbors() {
        let mut graph = EvidenceGraph::default();
        let core = NodeId::symbol("core", "core", "crate", "start");
        let helper = NodeId::symbol("core", "core", "crate", "helper");
        let dependency = NodeId::symbol("dependency", "dependency", "crate", "serve");
        let caller = NodeId::symbol("caller", "caller", "crate", "run");
        let transitive = NodeId::symbol("transitive", "transitive", "crate", "deeper");
        let unrelated = NodeId::symbol("unrelated", "unrelated", "crate", "idle");
        for id in [
            &core,
            &helper,
            &dependency,
            &caller,
            &transitive,
            &unrelated,
        ] {
            graph.merge_node(Node::new(
                id.clone(),
                NodeKind::Function,
                &id.symbol,
                evidence(&id.symbol),
            ));
        }
        for (source, target) in [
            (&core, &helper),
            (&core, &dependency),
            (&caller, &core),
            (&dependency, &transitive),
        ] {
            graph.merge_edge(Edge::new(
                source.clone(),
                target.clone(),
                EdgeKind::Calls,
                evidence("call"),
            ));
        }
        graph.merge_edge(Edge::new(
            dependency.clone(),
            caller.clone(),
            EdgeKind::Sends,
            evidence("neighbor-neighbor"),
        ));

        let model = project(
            &graph,
            &ViewRequest {
                kind: ViewKind::Overview,
                package: Some("core".to_string()),
                module: None,
                scenario: None,
                lod: DetailLevel::Full,
                filter: ArchitectureFilter::default(),
            },
        );
        let visible = model
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();

        assert_eq!(visible, BTreeSet::from([core, helper, dependency, caller]));
        assert!(!visible.contains(&transitive));
        assert!(!visible.contains(&unrelated));
        assert!(
            model
                .edges
                .iter()
                .all(|edge| { edge.source.package == "core" || edge.target.package == "core" })
        );
    }

    #[test]
    fn package_projection_includes_neighbor_summaries_without_a_budget() {
        let mut graph = EvidenceGraph::default();
        let core_package = NodeId::package("core");
        let dependency_package = NodeId::package("dependency");
        for (id, label) in [(&core_package, "core"), (&dependency_package, "dependency")] {
            graph.merge_node(Node::new(
                id.clone(),
                NodeKind::Package,
                label,
                evidence(label),
            ));
        }
        for label in ["alpha", "beta", "gamma"] {
            graph.merge_node(Node::new(
                NodeId::symbol("core", "core", "crate", label),
                NodeKind::Function,
                label,
                evidence(label),
            ));
        }
        graph.merge_edge(Edge::new(
            core_package.clone(),
            dependency_package.clone(),
            EdgeKind::DependsOn,
            evidence("cargo"),
        ));

        let model = project(
            &graph,
            &ViewRequest {
                kind: ViewKind::Overview,
                package: Some("core".to_string()),
                module: None,
                scenario: None,
                lod: DetailLevel::Full,
                filter: ArchitectureFilter::default(),
            },
        );
        let visible = model
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            visible,
            BTreeSet::from([
                core_package,
                dependency_package,
                NodeId::symbol("core", "core", "crate", "alpha"),
                NodeId::symbol("core", "core", "crate", "beta"),
                NodeId::symbol("core", "core", "crate", "gamma"),
            ])
        );
    }
}
