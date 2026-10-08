//! The chain stage, and what both chain searches read.

use std::collections::BTreeSet;

use anyhow::Result;

use super::{
    arms,
    body::Arm,
    chain::{Alike, Side},
    config::ChainConfig,
    facts::{self, Facts},
    graph::{Graph, Node},
    minhash, pairs,
    report::{self, ChainReport, Coverage},
    resolve::Resolver,
};

/// Finds the long parallel chains among `(path, text)` sources.
pub(crate) fn detect(sources: &[(String, String)], config: &ChainConfig) -> Result<ChainReport> {
    let mut facts = facts::collect(sources)?;
    facts.add_dependency_aliases(&config.dependency_roots);
    let mut resolver = Resolver::new(&facts, config.max_dyn_targets);
    let graph = Graph::build(
        &facts,
        &mut resolver,
        config.depth,
        config.max_variant_handlers,
    );
    let methods = resolver.trait_methods();
    let shingles = facts
        .fns
        .iter()
        .map(|f| minhash::shingles(&f.tokens, config.shingle))
        .collect();
    let ctx = Ctx {
        config,
        shingles,
        facts: &facts,
        graph: &graph,
        resolver: &resolver,
    };
    let mut chains = arms::decision_chains(&ctx, &methods);
    chains.extend(pairs::pair_chains(&ctx));
    Ok(ChainReport {
        chains: report::long_chains(chains, config),
        coverage: ctx.coverage(),
    })
}

/// A side with what it is compared by.
pub(super) struct Region {
    pub(super) fns: BTreeSet<usize>,
    pub(super) shingles: BTreeSet<u64>,
    pub(super) side: Side,
}

/// What both chain searches read.
pub(super) struct Ctx<'a> {
    pub(super) config: &'a ChainConfig,
    pub(super) facts: &'a Facts,
    pub(super) graph: &'a Graph,
    pub(super) resolver: &'a Resolver<'a>,
    shingles: Vec<BTreeSet<u64>>,
}

impl Ctx<'_> {
    /// The most alike compared pair across two regions.
    pub(super) fn alike(&self, x: &BTreeSet<usize>, y: &BTreeSet<usize>) -> Option<Alike> {
        let mut best: Option<(f64, usize, usize)> = None;
        for &a in x {
            for &b in y {
                let similarity = self.similarity(a, b);
                if similarity > best.map_or(0.0, |(top, ..)| top) {
                    best = Some((similarity, a, b));
                }
            }
        }
        best.map(|(similarity, a, b)| Alike {
            similarity,
            functions: [self.label(Node::Fn(a)), self.label(Node::Fn(b))],
        })
    }

    fn coverage(&self) -> Coverage {
        let mut unreached_private: Vec<String> = self
            .facts
            .fns
            .iter()
            .enumerate()
            .filter(|(fid, f)| {
                !f.public
                    && f.trait_name.is_none()
                    && f.name != "main"
                    && self.graph.preds(Node::Fn(*fid)).next().is_none()
            })
            .map(|(fid, _)| format!("{} {}", self.location(fid, None), self.label(Node::Fn(fid))))
            .collect();
        unreached_private.sort();
        Coverage {
            unreached_private,
            functions: self.facts.fns.len(),
        }
    }

    pub(super) fn eligible(&self, fid: usize) -> bool {
        self.facts
            .fns
            .get(fid)
            .is_some_and(|f| f.tokens.len() >= self.config.min_tokens)
    }

    pub(super) fn label(&self, node: Node) -> String {
        let Some(f) = self.facts.fns.get(node.function()) else {
            return String::new();
        };
        let name = f
            .owner
            .as_ref()
            .and_then(facts::last_ident)
            .map_or_else(|| f.name.clone(), |owner| format!("{owner}::{}", f.name));
        match node {
            Node::Fn(_) => name,
            Node::Arm(_, arm) => {
                let label = f.body.arms.get(arm).map_or("", |arm| arm.label.as_str());
                format!("{name}#arm{label}")
            }
        }
    }

    pub(super) fn lines(&self, fid: usize) -> usize {
        self.facts
            .fns
            .get(fid)
            .map_or(0, |f| (f.place.end + 1).saturating_sub(f.place.line).max(1))
    }

    pub(super) fn location(&self, fid: usize, line: Option<usize>) -> String {
        self.facts.fns.get(fid).map_or_else(String::new, |f| {
            format!("{}:{}", f.place.file, line.unwrap_or(f.place.line))
        })
    }

    /// A side of `fns`, plus an arm's own code when the arm is given.
    pub(super) fn region(
        &self,
        entry: String,
        location: String,
        fns: BTreeSet<usize>,
        arm: Option<&Arm>,
    ) -> Region {
        let mut shingles: BTreeSet<u64> = fns
            .iter()
            .flat_map(|&fid| self.shingles_of(fid).iter().copied())
            .collect();
        let mut lines: usize = fns.iter().map(|&fid| self.lines(fid)).sum();
        let mut inline = false;
        if let Some(arm) = arm {
            lines += (arm.lines.1 + 1).saturating_sub(arm.lines.0);
            if let Some(tokens) = &arm.tokens {
                shingles.extend(minhash::shingles(tokens, self.config.shingle));
                inline = true;
            }
        }
        let depth = fns
            .iter()
            .map(|&start| self.graph.chain_depth(start, &fns))
            .max()
            .unwrap_or_default();
        let mut members: Vec<String> = fns.iter().map(|&fid| self.label(Node::Fn(fid))).collect();
        members.sort();
        Region {
            side: Side {
                entry,
                location,
                inline,
                lines,
                depth,
                members,
                functions: fns.len(),
            },
            fns,
            shingles,
        }
    }

    pub(super) fn shingles_of(&self, fid: usize) -> &BTreeSet<u64> {
        static EMPTY: BTreeSet<u64> = BTreeSet::new();
        self.shingles.get(fid).unwrap_or(&EMPTY)
    }

    /// Similarity of two compared functions; any other pair has none.
    pub(super) fn similarity(&self, x: usize, y: usize) -> f64 {
        let (Some(fx), Some(fy)) = (self.facts.fns.get(x), self.facts.fns.get(y)) else {
            return 0.0;
        };
        if !self.eligible(x) || !self.eligible(y) {
            return 0.0;
        }
        let lengths = (fx.tokens.len(), fy.tokens.len());
        minhash::similarity(lengths, self.shingles_of(x), self.shingles_of(y))
    }
}
