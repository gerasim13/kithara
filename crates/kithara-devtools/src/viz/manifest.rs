use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Serialize;

use super::{
    cli::VizArgs,
    filter::FilterSummary,
    graph::{Edge, EvidenceGraph, Node},
    mermaid::DiagramSet,
    report,
    scenario::RuntimeSummary,
    semantic::{SemanticState, SemanticSummary},
    view::DiagramModel,
};

const SCHEMA_VERSION: u32 = 3;

#[derive(Debug)]
pub(crate) struct ArtifactSet {
    pub(crate) document: PathBuf,
}

#[derive(Serialize)]
struct GraphSnapshot<'a> {
    schema_version: u32,
    nodes: Vec<&'a Node>,
    edges: Vec<&'a Edge>,
}

#[derive(Serialize)]
struct ArtifactManifest<'a> {
    schema_version: u32,
    revision: &'a str,
    status: &'static str,
    view: &'static str,
    lod: u8,
    package: Option<&'a str>,
    module: Option<&'a str>,
    visible_nodes: usize,
    visible_edges: usize,
    hidden_nodes: usize,
    collapsed_groups: usize,
    filters: &'a FilterSummary<'a>,
    partition: PartitionManifest<'a>,
    semantic: &'a SemanticSummary,
    runtime: &'a RuntimeSummary,
    files: BTreeMap<&'static str, &'static str>,
}

#[derive(Serialize)]
struct PartitionManifest<'a> {
    state: &'static str,
    covered_nodes: usize,
    pages: Vec<PartitionPage<'a>>,
}

#[derive(Serialize)]
struct PartitionPage<'a> {
    label: &'a str,
    file: &'a str,
    mermaid: &'a str,
    visible_nodes: usize,
}

pub(crate) struct ArtifactRequest<'a> {
    pub(crate) root: &'a Path,
    pub(crate) revision: &'a str,
    pub(crate) project: &'a str,
    pub(crate) args: &'a VizArgs,
    pub(crate) graph: &'a EvidenceGraph,
    pub(crate) model: &'a DiagramModel,
    pub(crate) diagrams: &'a DiagramSet,
    pub(crate) semantic: &'a SemanticSummary,
    pub(crate) runtime: &'a RuntimeSummary,
    pub(crate) filters: &'a FilterSummary<'a>,
}

pub(crate) fn write(request: &ArtifactRequest<'_>) -> Result<ArtifactSet> {
    let output = request
        .root
        .join("target/architecture")
        .join(request.revision);
    fs::create_dir_all(&output)
        .with_context(|| format!("create architecture output: {}", output.display()))?;

    write_text(&output.join("architecture.mmd"), &request.diagrams.index)?;
    if !request.diagrams.pages.is_empty() {
        fs::create_dir_all(output.join("contours"))
            .with_context(|| format!("create architecture contours: {}", output.display()))?;
    }
    for page in &request.diagrams.pages {
        write_text(&output.join(&page.mermaid_file), &page.mermaid)?;
        write_text(
            &output.join(&page.document_file),
            &format!(
                "# {}\n\n[Back to architecture index](../architecture.md)\n\n```mermaid\n{}```\n",
                page.label, page.mermaid
            ),
        )?;
    }
    let document = architecture_document(request);
    write_text(&output.join("architecture.md"), &document)?;

    let snapshot = GraphSnapshot {
        schema_version: SCHEMA_VERSION,
        nodes: request.graph.nodes().collect(),
        edges: request.graph.edges().collect(),
    };
    write_json(&output.join("graph.json"), &snapshot)?;
    write_json(&output.join("projection.json"), request.model)?;

    let manifest = ArtifactManifest {
        schema_version: SCHEMA_VERSION,
        revision: request.revision,
        status: overall_status(request.semantic, request.runtime),
        view: request.args.view.as_str(),
        lod: request.model.lod.as_u8(),
        package: request.args.krate.as_deref(),
        module: request.args.module.as_deref(),
        visible_nodes: request.model.nodes.len(),
        visible_edges: request.model.edges.len(),
        hidden_nodes: request.model.hidden_nodes,
        collapsed_groups: request.model.groups.len(),
        filters: request.filters,
        partition: PartitionManifest {
            state: if request.diagrams.pages.is_empty() {
                "single"
            } else {
                "partitioned"
            },
            covered_nodes: request.diagrams.covered_nodes,
            pages: request
                .diagrams
                .pages
                .iter()
                .map(|page| PartitionPage {
                    label: &page.label,
                    file: &page.document_file,
                    mermaid: &page.mermaid_file,
                    visible_nodes: page.visible_nodes,
                })
                .collect(),
        },
        semantic: request.semantic,
        runtime: request.runtime,
        files: BTreeMap::from([
            ("document", "architecture.md"),
            ("graph", "graph.json"),
            ("manifest", "manifest.json"),
            ("mermaid", "architecture.mmd"),
            ("projection", "projection.json"),
        ]),
    };
    write_json(&output.join("manifest.json"), &manifest)?;

    Ok(ArtifactSet {
        document: output.join("architecture.md"),
    })
}

fn architecture_document(request: &ArtifactRequest<'_>) -> String {
    let analysis = report::render(request.model);
    let evidence = evidence_summary(request);
    format!(
        "# {} Architecture\n\n\
         Status: **{}**\n\n\
         ## Architecture\n\n\
         ```mermaid\n\
         {}```\n\n\
         Visible nodes: {}. Visible edges: {}. Outside the selected projection: {}. \
         Collapsed cycles: {}. Semantic edges resolved: {}.\n\n\
         {}\
         {}\
         ## Limitations\n\n\
         - Call targets without semantic evidence remain syntax-derived candidates.\n\
         - Unknown dynamic calls remain recorded in `graph.json`; the overview omits them instead of guessing a target.\n\
         - Runtime evidence proves only the configured observation; absence from a trace does not prove a path is dead.\n",
        request.project,
        overall_status(request.semantic, request.runtime),
        request.diagrams.index,
        request.model.nodes.len(),
        request.model.edges.len(),
        request.model.hidden_nodes,
        request.model.groups.len(),
        request.semantic.resolved_edges,
        analysis,
        evidence,
    ) + &contour_links(request.diagrams)
}

fn contour_links(diagrams: &DiagramSet) -> String {
    if diagrams.pages.is_empty() {
        return String::new();
    }
    let mut output = String::from("\n## Contour diagrams\n\n");
    for page in &diagrams.pages {
        output.push_str(&format!(
            "- [{}]({}) — {} visible nodes\n",
            page.label, page.document_file, page.visible_nodes
        ));
    }
    output
}

fn evidence_summary(request: &ArtifactRequest<'_>) -> String {
    let mut output = format!(
        "## Evidence status\n\n- Semantic: `{}`; requested {}, prepared {}, resolved {} edges, skipped {}.\n",
        request.semantic.state.as_str(),
        request.semantic.requested_symbols,
        request.semantic.prepared_symbols,
        request.semantic.resolved_edges,
        request.semantic.skipped_symbols,
    );
    if request.runtime.scenarios.is_empty() {
        output.push_str("- Runtime: no configured or selected scenario evidence.\n\n");
    } else {
        for scenario in &request.runtime.scenarios {
            output.push_str(&format!(
                "- Runtime `{}`: `{}`; {} records, {} matched, {} unmatched.\n",
                scenario.name,
                scenario.state.as_str(),
                scenario.trace.records,
                scenario.trace.matched_records,
                scenario.trace.unmatched_records,
            ));
        }
        output.push('\n');
    }
    output
}

fn overall_status(semantic: &SemanticSummary, runtime: &RuntimeSummary) -> &'static str {
    if semantic.is_incomplete() || runtime.is_incomplete() {
        return "incomplete";
    }
    if semantic.state == SemanticState::Truncated || runtime.is_truncated() {
        return "truncated";
    }
    if runtime.has_runtime() && semantic.state == SemanticState::Unavailable {
        return "runtime-enriched";
    }
    match semantic.state {
        SemanticState::Complete => "complete",
        SemanticState::Truncated => "truncated",
        SemanticState::Unavailable => "static-only",
        SemanticState::TimedOut | SemanticState::Failed => "incomplete",
    }
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}
