//! Call graph over functions and the match arms that handle an enum variant.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry};

use super::{
    body::DecisionKind,
    facts::Facts,
    resolve::{Resolver, Targets, dedup},
};

/// A function, or a match arm that handles a variant built elsewhere: state
/// handed through the enum reaches the arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Node {
    Fn(usize),
    /// A function and an index into its arms.
    Arm(usize, usize),
}

impl Node {
    pub(super) const fn as_fn(self) -> Option<usize> {
        match self {
            Self::Fn(fid) => Some(fid),
            Self::Arm(..) => None,
        }
    }

    pub(super) const fn function(self) -> usize {
        match self {
            Self::Fn(fid) | Self::Arm(fid, _) => fid,
        }
    }
}

pub(super) struct Graph {
    arm_sites: BTreeMap<Node, Vec<usize>>,
    calls: HashMap<Node, Vec<usize>>,
    handlers: HashMap<(String, String), BTreeSet<Node>>,
    pred: HashMap<Node, BTreeSet<Node>>,
    succ: HashMap<Node, Vec<Node>>,
    cfg: Vec<Vec<String>>,
    targets: Vec<Vec<Targets>>,
    depth: usize,
}

impl Graph {
    /// Callers of `fid` up to `depth` steps back, with their distance.
    pub(super) fn ancestors(&self, fid: usize) -> BTreeMap<Node, usize> {
        let mut dist = BTreeMap::from([(Node::Fn(fid), 0)]);
        let mut frontier = vec![Node::Fn(fid)];
        for step in 1..=self.depth {
            let mut next = Vec::new();
            for node in frontier {
                for caller in self.preds(node) {
                    if let Entry::Vacant(entry) = dist.entry(caller) {
                        entry.insert(step);
                        next.push(caller);
                    }
                }
            }
            frontier = next;
        }
        dist
    }

    /// Resolves every site and links the variants a function builds to the
    /// match arms that handle them. A variant handled in more than
    /// `max_handlers` functions is a hub and links nowhere.
    pub(super) fn build<'f>(
        facts: &'f Facts,
        resolver: &mut Resolver<'f>,
        depth: usize,
        max_handlers: usize,
    ) -> Self {
        let targets = facts
            .fns
            .iter()
            .enumerate()
            .map(|(fid, f)| {
                f.body
                    .sites
                    .iter()
                    .map(|site| resolver.site_targets(fid, site))
                    .collect()
            })
            .collect();
        let mut arm_sites: BTreeMap<Node, Vec<usize>> = BTreeMap::new();
        let mut handlers: HashMap<(String, String), BTreeSet<Node>> = HashMap::new();
        for (fid, f) in facts.fns.iter().enumerate() {
            for (index, site) in f.body.sites.iter().enumerate() {
                for &frame in &site.frames {
                    let Some(arm) = f
                        .body
                        .arms
                        .get(frame)
                        .filter(|arm| arm.kind == DecisionKind::Match)
                    else {
                        continue;
                    };
                    let node = Node::Arm(fid, frame);
                    let sites = arm_sites.entry(node).or_default();
                    if sites.is_empty() {
                        for key in arm
                            .variants
                            .iter()
                            .filter_map(|path| resolver.variant(fid, path))
                        {
                            handlers.entry(key).or_default().insert(node);
                        }
                    }
                    sites.push(index);
                }
            }
        }
        handlers.retain(|_, arms| {
            arms.iter()
                .map(|arm| arm.function())
                .collect::<BTreeSet<_>>()
                .len()
                <= max_handlers
        });
        let mut graph = Self {
            targets,
            handlers,
            arm_sites,
            depth,
            succ: HashMap::new(),
            calls: HashMap::new(),
            pred: HashMap::new(),
            cfg: Vec::new(),
        };
        let nodes: Vec<Node> = (0..facts.fns.len())
            .map(Node::Fn)
            .chain(graph.arm_sites.keys().copied())
            .collect();
        for node in nodes {
            let (succ, calls) = graph.edges(node);
            for &callee in &succ {
                graph.pred.entry(callee).or_default().insert(node);
            }
            graph.succ.insert(node, succ);
            graph.calls.insert(node, calls);
        }
        graph.cfg = graph.effective_cfg(facts);
        graph
    }

    fn call_successors(&self, node: Node) -> &[usize] {
        self.calls.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Whether `from` reaches `target` through at most `depth` direct calls,
    /// never passing `blocked` or any node of the function `fence`. An arm
    /// starts from the functions it calls.
    pub(super) fn calls_into(
        &self,
        from: Node,
        target: usize,
        blocked: usize,
        fence: usize,
    ) -> bool {
        let start: Vec<usize> = match from {
            Node::Fn(fid) if fid == target => return true,
            Node::Fn(fid) if fid == blocked => return false,
            Node::Fn(fid) => vec![fid],
            Node::Arm(..) => {
                let callees = self.call_successors(from).iter().copied();
                callees
                    .filter(|callee| *callee != blocked && *callee != fence)
                    .collect()
            }
        };
        if start.contains(&target) {
            return true;
        }
        let mut seen: HashSet<usize> = start.iter().copied().chain([blocked]).collect();
        let mut frontier = start;
        for _ in 0..self.depth {
            let mut next = Vec::new();
            for node in frontier {
                for &callee in self.call_successors(Node::Fn(node)) {
                    if callee == fence || !seen.insert(callee) {
                        continue;
                    }
                    if callee == target {
                        return true;
                    }
                    next.push(callee);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        false
    }

    pub(super) fn cfg(&self, fid: usize) -> &[String] {
        self.cfg.get(fid).map_or(&[], Vec::as_slice)
    }

    /// Both functions build only under cfgs, and the cfgs differ.
    pub(super) fn cfg_split(&self, x: usize, y: usize) -> bool {
        let (cx, cy) = (self.cfg(x), self.cfg(y));
        !cx.is_empty() && !cy.is_empty() && cx != cy
    }

    /// Longest call chain from `start` through functions of `region`; arms on
    /// the way add no length.
    pub(super) fn chain_depth(&self, start: usize, region: &BTreeSet<usize>) -> usize {
        let mut dist = HashMap::from([(Node::Fn(start), 0)]);
        let mut frontier = vec![Node::Fn(start)];
        let mut deepest = 0;
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for node in frontier {
                let base = dist.get(&node).copied().unwrap_or_default();
                for &callee in self.successors(node) {
                    if dist.contains_key(&callee)
                        || callee.as_fn().is_some_and(|fid| !region.contains(&fid))
                    {
                        continue;
                    }
                    let step = usize::from(callee.as_fn().is_some());
                    dist.insert(callee, base + step);
                    if step > 0 {
                        deepest = deepest.max(base + step);
                    }
                    next.push(callee);
                }
            }
            frontier = next;
        }
        deepest
    }

    fn edges(&self, node: Node) -> (Vec<Node>, Vec<usize>) {
        let fid = node.function();
        let sites = match node {
            Node::Fn(_) => (0..self.targets.get(fid).map_or(0, Vec::len)).collect(),
            Node::Arm(..) => self.arm_sites.get(&node).cloned().unwrap_or_default(),
        };
        let mut succ = Vec::new();
        let mut calls = Vec::new();
        for site in sites {
            calls.extend_from_slice(self.site_calls(fid, site));
            succ.extend(self.site_nodes(fid, site).filter(|callee| *callee != node));
        }
        (dedup(succ), dedup(calls))
    }

    /// A function without cfg of its own takes the one cfg all its callers share.
    fn effective_cfg(&self, facts: &Facts) -> Vec<Vec<String>> {
        let mut cfg: Vec<Vec<String>> = facts
            .fns
            .iter()
            .map(|f| {
                let mut own = f.cfg.clone();
                own.sort();
                own
            })
            .collect();
        for _ in 0..self.depth {
            let mut changed = false;
            for fid in 0..cfg.len() {
                if cfg.get(fid).is_none_or(|own| !own.is_empty()) {
                    continue;
                }
                let callers: BTreeSet<&Vec<String>> = self
                    .preds(Node::Fn(fid))
                    .filter_map(Node::as_fn)
                    .filter_map(|caller| cfg.get(caller))
                    .collect();
                let inherited = match callers.first() {
                    Some(shared) if callers.len() == 1 && !shared.is_empty() => (*shared).clone(),
                    _ => continue,
                };
                if let Some(slot) = cfg.get_mut(fid) {
                    *slot = inherited;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        cfg
    }

    /// Functions only the region calls into: grown from `seeds` while every
    /// caller of a successor already sits inside. A match arm sits inside
    /// with its function. `exclude` never joins.
    pub(super) fn owned(&self, seeds: &[Node], exclude: Option<usize>) -> BTreeSet<usize> {
        let mut region: HashSet<Node> = seeds.iter().copied().collect();
        let mut frontier = seeds.to_vec();
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for node in frontier {
                for &callee in self.successors(node) {
                    if region.contains(&callee) || exclude.map(Node::Fn) == Some(callee) {
                        continue;
                    }
                    if self.preds(callee).all(|caller| {
                        let host = Node::Fn(caller.function());
                        host == callee || region.contains(&host) || region.contains(&caller)
                    }) {
                        region.insert(callee);
                        next.push(callee);
                    }
                }
            }
            frontier = next;
        }
        region.into_iter().filter_map(Node::as_fn).collect()
    }

    pub(super) fn preds(&self, node: Node) -> impl Iterator<Item = Node> + '_ {
        self.pred.get(&node).into_iter().flatten().copied()
    }

    /// Functions a site calls directly.
    pub(super) fn site_calls(&self, fid: usize, site: usize) -> &[usize] {
        self.targets
            .get(fid)
            .and_then(|sites| sites.get(site))
            .map_or(&[], |targets| targets.fns.as_slice())
    }

    /// Functions a site calls and the match arms that handle the variant it builds.
    pub(super) fn site_nodes(&self, fid: usize, site: usize) -> impl Iterator<Item = Node> + '_ {
        let targets = self.targets.get(fid).and_then(|sites| sites.get(site));
        let calls = targets
            .into_iter()
            .flat_map(|targets| targets.fns.iter().copied().map(Node::Fn));
        let handled = targets
            .and_then(|targets| targets.variant.as_ref())
            .and_then(|variant| self.handlers.get(variant))
            .into_iter()
            .flatten()
            .copied();
        calls.chain(handled)
    }

    pub(super) fn successors(&self, node: Node) -> &[Node] {
        self.succ.get(&node).map_or(&[], Vec::as_slice)
    }
}
