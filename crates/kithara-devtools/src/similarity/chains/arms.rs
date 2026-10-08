//! Chains found from a decision: each arm grows the region only it calls into,
//! and every two alike arms form a row. The impls of one trait method are the
//! arms of the call that picks among them.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::{
    body::BodyFacts,
    chain::{Chain, Fork, Origin, Split},
    facts::FnFact,
    graph::Node,
    search::{Ctx, Region},
};

/// Impls of one workspace trait method, as the resolver found them.
pub(super) struct TraitMethod {
    pub(super) name: String,
    pub(super) impls: Vec<usize>,
}

pub(super) fn decision_chains(ctx: &Ctx, methods: &[TraitMethod]) -> Vec<Chain> {
    let mut chains = Vec::new();
    for (fid, f) in ctx.facts.fns.iter().enumerate() {
        decisions(ctx, fid, f, &mut chains);
    }
    dyn_chains(ctx, methods, &mut chains);
    chains
}

fn decisions(ctx: &Ctx, fid: usize, f: &FnFact, chains: &mut Vec<Chain>) {
    let body = &f.body;
    let mut arm_sites: Vec<Vec<usize>> = vec![Vec::new(); body.arms.len()];
    for (index, site) in body.sites.iter().enumerate() {
        for &frame in &site.frames {
            if let Some(sites) = arm_sites.get_mut(frame) {
                sites.push(index);
            }
        }
    }
    let mut decisions: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, arm) in body.arms.iter().enumerate() {
        decisions.entry(arm.decision).or_default().push(index);
    }
    let fork = Fork {
        function: ctx.label(Node::Fn(fid)),
        location: ctx.location(fid, None),
    };
    for arms in decisions.values().filter(|arms| arms.len() >= 2) {
        let sides: Vec<(Region, &[String])> = arms
            .iter()
            .filter_map(|&arm| {
                let sites = arm_sites.get(arm).map_or(&[][..], Vec::as_slice);
                arm_side(ctx, fid, body, arm, sites)
            })
            .collect();
        let kind = arms
            .first()
            .and_then(|&arm| body.arms.get(arm))
            .map(|arm| arm.kind);
        let Some(kind) = kind else {
            continue;
        };
        for (index, (x, cfg_x)) in sides.iter().enumerate() {
            for (y, cfg_y) in sides.iter().skip(index + 1) {
                let platform = platform_split(ctx, cfg_x, cfg_y, &x.fns, &y.fns);
                let split = Split::Decision(kind);
                if let Some(chain) = emit(
                    ctx,
                    Origin::Decision,
                    split,
                    fork.clone(),
                    [x, y],
                    platform,
                    None,
                ) {
                    chains.push(chain);
                }
            }
        }
    }
}

/// The region an arm alone reaches: the targets of its own sites that no
/// other site of the function reaches and only the function calls, grown
/// through their callees. The arm's cfg comes along.
fn arm_side<'b>(
    ctx: &Ctx,
    fid: usize,
    body: &'b BodyFacts,
    arm: usize,
    sites: &[usize],
) -> Option<(Region, &'b [String])> {
    let info = body.arms.get(arm)?;
    let graph = ctx.graph;
    let inside: HashSet<usize> = sites.iter().copied().collect();
    let reached: BTreeSet<Node> = sites
        .iter()
        .flat_map(|&site| graph.site_nodes(fid, site))
        .collect();
    let outside: HashSet<Node> = (0..body.sites.len())
        .filter(|site| !inside.contains(site))
        .flat_map(|site| graph.site_nodes(fid, site))
        .collect();
    let seeds: Vec<Node> = reached
        .into_iter()
        .filter(|node| !outside.contains(node) && *node != Node::Fn(fid))
        .filter(|&node| graph.preds(node).all(|caller| caller.function() == fid))
        .collect();
    let fns = graph.owned(&seeds, None);
    let location = ctx.location(fid, Some(info.lines.0));
    let region = ctx.region(info.label.clone(), location, fns, Some(info));
    (region.side.units() > 0).then_some((region, info.cfg.as_slice()))
}

/// Arms under different cfgs, or regions that each build only under cfgs no
/// function of the other side shares.
fn platform_split(
    ctx: &Ctx,
    cfg_x: &[String],
    cfg_y: &[String],
    x: &BTreeSet<usize>,
    y: &BTreeSet<usize>,
) -> bool {
    if !cfg_x.is_empty() && !cfg_y.is_empty() && cfg_x != cfg_y {
        return true;
    }
    let cfgs = |fns: &BTreeSet<usize>| -> BTreeSet<&[String]> {
        fns.iter().map(|&fid| ctx.graph.cfg(fid)).collect()
    };
    let (cx, cy) = (cfgs(x), cfgs(y));
    let bound =
        |cfgs: &BTreeSet<&[String]>| !cfgs.is_empty() && cfgs.iter().all(|cfg| !cfg.is_empty());
    bound(&cx) && bound(&cy) && cx.is_disjoint(&cy)
}

/// A row for two sides with enough units; impls of one trait method must
/// also be alike, since impls differ by design.
fn emit(
    ctx: &Ctx,
    origin: Origin,
    split: Split,
    fork: Fork,
    [x, y]: [&Region; 2],
    platform: bool,
    built: Option<[Vec<String>; 2]>,
) -> Option<Chain> {
    if x.side.units() + y.side.units() < ctx.config.min_units {
        return None;
    }
    let alike = ctx.alike(&x.fns, &y.fns);
    let mut chain = Chain::new(origin, split, Some(fork), [x, y], alike);
    let best = chain.alike.as_ref().map_or(0.0, |alike| alike.similarity);
    if split == Split::Dyn && chain.jaccard < ctx.config.dyn_jaccard && best < ctx.config.dyn_pair {
        return None;
    }
    chain.platform = platform;
    chain.built = built;
    Some(chain)
}

/// Every two impls of a trait method that do not delegate to each other.
fn dyn_chains(ctx: &Ctx, methods: &[TraitMethod], chains: &mut Vec<Chain>) {
    let built = built_at(ctx);
    for method in methods {
        let regions: Vec<(usize, Region)> = method
            .impls
            .iter()
            .map(|&fid| {
                let fns = ctx.graph.owned(&[Node::Fn(fid)], None);
                (
                    fid,
                    ctx.region(ctx.label(Node::Fn(fid)), ctx.location(fid, None), fns, None),
                )
            })
            .collect();
        for (index, (x, rx)) in regions.iter().enumerate() {
            for (y, ry) in regions.iter().skip(index + 1) {
                if !rx.fns.is_disjoint(&ry.fns) {
                    continue;
                }
                let fork = Fork {
                    function: method.name.clone(),
                    location: ctx.location(*x, None),
                };
                let sites = [*x, *y].map(|fid| {
                    ctx.resolver
                        .owner_keys(fid)
                        .iter()
                        .flat_map(|owner| built.get(owner).into_iter().flatten().cloned())
                        .collect()
                });
                let platform = ctx.graph.cfg_split(*x, *y);
                if let Some(chain) = emit(
                    ctx,
                    Origin::Dyn,
                    Split::Dyn,
                    fork,
                    [rx, ry],
                    platform,
                    Some(sites),
                ) {
                    chains.push(chain);
                }
            }
        }
    }
}

/// Functions that build a value of each type: `Type { .. }` or `Type::new(..)`.
fn built_at(ctx: &Ctx) -> BTreeMap<super::ty::ItemPath, BTreeSet<String>> {
    let mut built: BTreeMap<super::ty::ItemPath, BTreeSet<String>> = BTreeMap::new();
    for (fid, f) in ctx.facts.fns.iter().enumerate() {
        for (index, _) in f.body.sites.iter().enumerate() {
            for key in ctx.graph.built(fid, index) {
                built
                    .entry(key.clone())
                    .or_default()
                    .insert(ctx.label(Node::Fn(fid)));
            }
        }
    }
    built
}
