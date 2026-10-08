//! Chains found from two alike roots: the roots themselves, or the regions they
//! own, are alike. The fork is found afterwards in their nearest common caller.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{
    body::BodyFacts,
    chain::{Alike, Chain, Fork, Origin, Split},
    graph::Node,
    minhash::{candidates, jaccard},
    search::Ctx,
};

struct Finding {
    origin: Origin,
    owned: [BTreeSet<usize>; 2],
    similarity: f64,
    x: usize,
    y: usize,
}

pub(super) fn pair_chains(ctx: &Ctx) -> Vec<Chain> {
    let mut pairs = root_pairs(ctx);
    region_pairs(ctx, &mut pairs);
    let findings: Vec<Finding> = pairs
        .into_iter()
        .filter_map(|((x, y), (similarity, origin))| finding(ctx, x, y, similarity, origin))
        .collect();
    unfolded(&findings).map(|found| chain(ctx, found)).collect()
}

/// Compared functions whose own tokens are alike.
fn root_pairs(ctx: &Ctx) -> BTreeMap<(usize, usize), (f64, Origin)> {
    let eligible: Vec<(usize, &BTreeSet<u64>)> = (0..ctx.facts.fns.len())
        .filter(|&fid| ctx.eligible(fid))
        .map(|fid| (fid, ctx.shingles_of(fid)))
        .collect();
    candidates(&eligible)
        .into_iter()
        .filter(|&(x, y)| !ctx.graph.cfg_split(x, y))
        .filter_map(|(x, y)| {
            let similarity = ctx.similarity(x, y);
            (similarity >= ctx.config.similarity).then_some(((x, y), (similarity, Origin::Pair)))
        })
        .collect()
}

/// Roots whose owned regions are alike even when the roots differ.
fn region_pairs(ctx: &Ctx, pairs: &mut BTreeMap<(usize, usize), (f64, Origin)>) {
    let mut roots: BTreeMap<usize, (BTreeSet<usize>, BTreeSet<u64>)> = BTreeMap::new();
    for fid in 0..ctx.facts.fns.len() {
        let region = ctx.graph.owned(&[Node::Fn(fid)], None);
        let lines: usize = region.iter().map(|&member| ctx.lines(member)).sum();
        if region.len() >= 2 && lines >= ctx.config.min_side_lines {
            let shingles = region
                .iter()
                .flat_map(|&member| ctx.shingles_of(member).iter().copied())
                .collect();
            roots.insert(fid, (region, shingles));
        }
    }
    let sets: Vec<(usize, &BTreeSet<u64>)> = roots
        .iter()
        .map(|(&fid, (_, shingles))| (fid, shingles))
        .collect();
    for (x, y) in candidates(&sets) {
        let (Some((rx, sx)), Some((ry, sy))) = (roots.get(&x), roots.get(&y)) else {
            continue;
        };
        if pairs.contains_key(&(x, y)) || ctx.graph.cfg_split(x, y) || !rx.is_disjoint(ry) {
            continue;
        }
        let similarity = jaccard(sx, sy);
        if similarity >= ctx.config.similarity {
            pairs.insert((x, y), (similarity, Origin::Region));
        }
    }
}

/// A pair that owns enough code, where neither root calls the other and the
/// roots are not impls of one trait method.
fn finding(ctx: &Ctx, x: usize, y: usize, similarity: f64, origin: Origin) -> Option<Finding> {
    let graph = ctx.graph;
    if graph.successors(Node::Fn(x)).contains(&Node::Fn(y))
        || graph.successors(Node::Fn(y)).contains(&Node::Fn(x))
    {
        return None;
    }
    let (fx, fy) = (ctx.facts.fns.get(x)?, ctx.facts.fns.get(y)?);
    if !ctx.resolver.trait_keys(x).is_empty()
        && ctx.resolver.trait_keys(x) == ctx.resolver.trait_keys(y)
        && fx.name == fy.name
    {
        return None;
    }
    let owned = [
        graph.owned(&[Node::Fn(x)], Some(y)),
        graph.owned(&[Node::Fn(y)], Some(x)),
    ];
    (owned.iter().map(BTreeSet::len).sum::<usize>() >= ctx.config.min_units).then_some(Finding {
        origin,
        owned,
        similarity,
        x,
        y,
    })
}

/// Findings whose roots do not sit on the two sides of another finding.
fn unfolded(findings: &[Finding]) -> impl Iterator<Item = &Finding> {
    let mut holders: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (index, found) in findings.iter().enumerate() {
        for (side, owned) in found.owned.iter().enumerate() {
            for &fid in owned {
                holders.entry(fid).or_default().push((index, side));
            }
        }
    }
    findings
        .iter()
        .enumerate()
        .filter_map(move |(index, found)| {
            let nested = holders
                .get(&found.x)
                .into_iter()
                .flatten()
                .any(|&(other, side)| {
                    other != index
                        && findings
                            .get(other)
                            .and_then(|outer| outer.owned.get(1 - side))
                            .is_some_and(|opposite| opposite.contains(&found.y))
                });
            (!nested).then_some(found)
        })
}

fn chain(ctx: &Ctx, found: &Finding) -> Chain {
    let (split, fork) = fork(ctx, found.x, found.y);
    let [ox, oy] = found.owned.clone();
    let regions = [
        ctx.region(
            ctx.label(Node::Fn(found.x)),
            ctx.location(found.x, None),
            ox,
            None,
        ),
        ctx.region(
            ctx.label(Node::Fn(found.y)),
            ctx.location(found.y, None),
            oy,
            None,
        ),
    ];
    let alike = Alike {
        functions: [ctx.label(Node::Fn(found.x)), ctx.label(Node::Fn(found.y))],
        similarity: found.similarity,
    };
    Chain::new(
        found.origin,
        split,
        fork,
        [&regions[0], &regions[1]],
        Some(alike),
    )
}

/// The nearest common caller and what in it separates the two roots, at the
/// line where the split shows; the most specific split wins among callers at
/// one distance.
fn fork(ctx: &Ctx, x: usize, y: usize) -> (Split, Option<Fork>) {
    let (ax, ay) = (ctx.graph.ancestors(x), ctx.graph.ancestors(y));
    let common: Vec<(usize, Node)> = ax
        .iter()
        .filter(|(node, _)| **node != Node::Fn(x) && **node != Node::Fn(y))
        .filter_map(|(node, dx)| ay.get(node).map(|dy| (dx + dy, *node)))
        .collect();
    let Some(nearest) = common.iter().map(|(distance, _)| *distance).min() else {
        return (Split::Entry, None);
    };
    let at_nearest = common.iter().filter(|(distance, _)| *distance == nearest);
    let best = at_nearest
        .map(|&(_, node)| (attribute(ctx, node, x, y), node))
        .min_by_key(|((split, _), node)| (specificity(*split), *node));
    best.map_or((Split::Entry, None), |((split, line), node)| {
        let fid = node.function();
        let fork = Fork {
            function: ctx.label(Node::Fn(fid)),
            location: ctx.location(fid, line),
        };
        (split, Some(fork))
    })
}

const fn specificity(split: Split) -> u8 {
    match split {
        Split::Decision(_) | Split::Fallback(_) | Split::Dyn => 0,
        Split::Sequence => 1,
        Split::ViaCallee | Split::Entry => 2,
    }
}

/// Which sites of the common caller reach only `x` or only `y`, what
/// construct first separates such two sites, and the earlier line of them.
fn attribute(ctx: &Ctx, node: Node, x: usize, y: usize) -> (Split, Option<usize>) {
    let graph = ctx.graph;
    let fid = node.function();
    let Some(body) = ctx.facts.fns.get(fid).map(|f| &f.body) else {
        return (Split::ViaCallee, None);
    };
    let (mut only_x, mut only_y, mut both) = (Vec::new(), Vec::new(), None);
    for (index, site) in body.sites.iter().enumerate() {
        let nodes: Vec<Node> = graph
            .site_nodes(fid, index)
            .filter(|callee| callee.function() != fid)
            .collect();
        let hits_x = nodes
            .iter()
            .any(|&callee| graph.calls_into(callee, x, y, fid));
        let hits_y = nodes
            .iter()
            .any(|&callee| graph.calls_into(callee, y, x, fid));
        match (hits_x, hits_y) {
            (true, false) => only_x.push(site),
            (false, true) => only_y.push(site),
            (true, true) if graph.site_calls(fid, index).len() > 1 => {
                both = both.or(Some(site.line));
            }
            (true, true) | (false, false) => {}
        }
    }
    if only_x.is_empty() && only_y.is_empty() && both.is_some() {
        return (Split::Dyn, both);
    }
    let mut best: Option<(Split, usize)> = None;
    for site_x in &only_x {
        for site_y in &only_y {
            let split = diverge(body, &site_x.frames, &site_y.frames).unwrap_or(Split::Sequence);
            if best.is_none_or(|(found, _)| found == Split::Sequence && split != Split::Sequence) {
                best = Some((split, site_x.line.min(site_y.line)));
            }
        }
    }
    best.map_or((Split::ViaCallee, None), |(split, line)| {
        (split, Some(line))
    })
}

/// The decision whose different arms hold the two sites, or the arm that
/// fails into the site nested one level deeper.
fn diverge(body: &BodyFacts, x: &[usize], y: &[usize]) -> Option<Split> {
    for (&ax, &ay) in x.iter().zip(y) {
        let (arm_x, arm_y) = (body.arms.get(ax)?, body.arms.get(ay)?);
        if arm_x.decision != arm_y.decision {
            return None;
        }
        if ax != ay {
            return Some(Split::Decision(arm_x.kind));
        }
    }
    let (short, long) = if x.len() < y.len() { (x, y) } else { (y, x) };
    let deeper = body.arms.get(*long.get(short.len())?)?;
    deeper.fail.then_some(Split::Fallback(deeper.kind))
}
