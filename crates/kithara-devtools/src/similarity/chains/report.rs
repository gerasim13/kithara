//! Which chains are long enough to list, and their section of the report.

use std::{cmp::Reverse, collections::HashSet};

use serde::Serialize;

use super::{
    chain::{Chain, Origin, Side},
    config::ChainConfig,
};

/// The long chains of a source set, and how much of it the call graph reaches.
#[derive(Debug, Serialize)]
pub(crate) struct ChainReport {
    pub(crate) coverage: Coverage,
    pub(crate) chains: Vec<Chain>,
}

/// Private functions no resolved call reaches: each is a call the resolver
/// missed, or dead code.
#[derive(Debug, Serialize)]
pub(crate) struct Coverage {
    pub(crate) unreached_private: Vec<String>,
    pub(crate) functions: usize,
}

/// Decision rows come before pairs, and the shorter side ranks them. A row
/// over the functions of a listed row is dropped; every row needs enough units
/// and lines on each side, and a decision row needs alike sides.
pub(super) fn long_chains(chains: Vec<Chain>, config: &ChainConfig) -> Vec<Chain> {
    let mut rows: Vec<Chain> = chains
        .into_iter()
        .filter(|chain| match chain.origin {
            Origin::Decision | Origin::Dyn => {
                chain.jaccard >= config.arm_jaccard || chain.containment >= config.arm_containment
            }
            Origin::Pair | Origin::Region => true,
        })
        .collect();
    rows.sort_by_key(|chain| Reverse(shorter(chain)));
    let mut seen = HashSet::new();
    rows.retain(|chain| {
        seen.insert(chain.fns.clone())
            && chain.sides.iter().map(Side::units).sum::<usize>() >= config.min_units
            && shorter(chain) >= config.min_side_lines
    });
    rows
}

fn shorter(chain: &Chain) -> usize {
    chain
        .sides
        .iter()
        .map(|side| side.lines)
        .min()
        .unwrap_or_default()
}

/// The `## Parallel chains` section: one table row per chain, and the
/// coverage of the call graph.
pub(crate) fn markdown(report: &ChainReport) -> String {
    let coverage = &report.coverage;
    let mut output = format!(
        "## Parallel chains\n\n\
         - Chains: {}\n\
         - Functions: {}; private functions no resolved call reaches: {} (listed in \
         `chains.json`)\n\n",
        report.chains.len(),
        coverage.functions,
        coverage.unreached_private.len(),
    );
    if report.chains.is_empty() {
        output.push_str("No long parallel chains were found.\n");
        return output;
    }
    output.push_str(
        "| # | Split | Units | Lines | Depth | Jaccard | Containment | Sides | Alike | Fork |\n\
         |---:|---|---|---|---|---:|---:|---|---|---|\n",
    );
    for (index, chain) in report.chains.iter().enumerate() {
        let [x, y] = &chain.sides;
        let platform = if chain.platform { " platform" } else { "" };
        let alike = chain.alike.as_ref().map_or_else(String::new, |alike| {
            let [a, b] = &alike.functions;
            format!("`{a}` / `{b}` {:.0}%", alike.similarity * 100.0)
        });
        let fork = chain.fork.as_ref().map_or_else(String::new, |fork| {
            format!("`{}` `{}`", fork.function, fork.location)
        });
        output.push_str(&format!(
            "| {} | `{}`{platform} | {}/{} | {}/{} | {}/{} | {:.0}% | {:.0}% | `{}` `{}` / `{}` `{}` | {alike} | {fork} |\n",
            index + 1,
            chain.split,
            x.units(),
            y.units(),
            x.lines,
            y.lines,
            x.depth,
            y.depth,
            chain.jaccard * 100.0,
            chain.containment * 100.0,
            x.entry,
            x.location,
            y.entry,
            y.location,
        ));
    }
    output
}
