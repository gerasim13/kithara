use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result};
use serde::Serialize;
use syn::{Fields, GenericParam, ImplItem, Item};

use super::{
    SimilarityConfig,
    behavior::BehaviorGraph,
    catalog::TypeCatalog,
    cfg::{Predicate, module_predicates},
    config::is_test_path,
    shape::{ComparisonCache, ShapeArena, ShapeId, TypeSubstitution},
};
use crate::consts;

#[derive(Debug, Default, Serialize)]
pub(super) struct AnalysisReport {
    pub(super) candidates: Vec<Candidate>,
    pub(super) schema_version: u32,
    pub(super) abstractions: usize,
    pub(super) cached_comparisons: usize,
    pub(super) interned_shapes: usize,
    pub(super) scanned_files: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct Candidate {
    pub(super) left: AbstractionRef,
    pub(super) right: AbstractionRef,
    pub(super) behavior_similarity: Option<f64>,
    pub(super) recommendation: Recommendation,
    pub(super) field_matches: Vec<FieldMatch>,
    pub(super) shared_behavior: Vec<String>,
    pub(super) substitutions: Vec<TypeSubstitution>,
    pub(super) overall_similarity: f64,
    pub(super) state_similarity: f64,
}

impl Candidate {
    pub(super) fn is_twin(&self, min_behavior: f64) -> bool {
        self.left.kind == AbstractionKind::Struct
            && self.right.kind == AbstractionKind::Struct
            && self.recommendation == Recommendation::Merge
            && self
                .behavior_similarity
                .is_some_and(|score| score >= min_behavior)
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AbstractionRef {
    pub(super) kind: AbstractionKind,
    pub(super) name: String,
    pub(super) path: String,
    pub(super) line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Recommendation {
    ExtractComponent,
    ExtractGenericFunction,
    GenericParameter,
    Merge,
    ReviewPartialState,
    ReviewStructuralOverlap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum AbstractionKind {
    Function,
    Struct,
}

#[derive(Debug, Serialize)]
pub(super) struct FieldMatch {
    pub(super) left: String,
    pub(super) right: String,
    pub(super) match_similarity: f64,
    pub(super) name_similarity: f64,
    pub(super) type_similarity: f64,
}

#[cfg(test)]
pub(super) fn analyze_source(path: &str, source: &str) -> Result<AnalysisReport> {
    analyze_source_with_config(path, source, &SimilarityConfig::default())
}

#[cfg(test)]
fn analyze_source_with_config(
    path: &str,
    source: &str,
    config: &SimilarityConfig,
) -> Result<AnalysisReport> {
    analyze_sources(
        &[(path.to_string(), source.to_string())],
        &[],
        config,
        false,
    )
}

/// Struct pairs that cannot be folded: cfg alternatives never compiled together,
/// and all-scalar structs in crates with no dependency edge between them.
fn never_twins(left: &Abstraction, right: &Abstraction, config: &SimilarityConfig) -> bool {
    let depends = |from: &str, to: &str| {
        config
            .workspace_dependencies
            .get(from)
            .is_some_and(|dependencies| dependencies.contains(to))
    };
    let (left_crate, right_crate) = (crate_from_path(&left.path), crate_from_path(&right.path));
    !left.cfg.can_coexist(&right.cfg)
        || left.primitive_fields
            && right.primitive_fields
            && left_crate != right_crate
            && !depends(left_crate, right_crate)
            && !depends(right_crate, left_crate)
}

pub(super) fn analyze_sources(
    sources: &[(String, String)],
    module_sources: &[(String, String)],
    config: &SimilarityConfig,
    include_tests: bool,
) -> Result<AnalysisReport> {
    let catalog = TypeCatalog::from_config(config)?;
    let mut collected = Collector::default();
    let files = sources
        .iter()
        .chain(module_sources)
        .map(|(path, source)| {
            let file = syn::parse_file(source)
                .with_context(|| format!("parse Rust source for similarity: {path}"))?;
            Ok((path.clone(), file))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let modules = module_predicates(&files)?;
    for (path, _) in sources {
        let Some(file) = files.get(path) else {
            continue;
        };
        let effective = match modules.get(path) {
            Some(predicate) => predicate.clone(),
            None => Predicate::from_attributes(&file.attrs)?,
        };
        if !include_tests && (is_test_path(Path::new(path)) || effective.is_test_only()) {
            continue;
        }
        collected.collect_module(&file.items, "", path, &effective, include_tests)?;
    }
    let Collector {
        arena,
        mut abstractions,
        pending_impls,
    } = collected;
    attach_impls(&mut abstractions, pending_impls);
    let mut cache = ComparisonCache::default();
    let mut candidates = Vec::new();
    let pairs = candidate_pairs(&abstractions, &arena, &catalog);
    for (left_index, right_index) in pairs {
        let left = &abstractions[left_index];
        let right = &abstractions[right_index];
        if left.kind == AbstractionKind::Struct && never_twins(left, right, config) {
            continue;
        }
        if left.kind == AbstractionKind::Function {
            let Some(behavior) = compare_behaviors(&left.behaviors, &right.behaviors)
                .filter(|behavior| behavior.score >= 0.75)
            else {
                continue;
            };
            candidates.push(Candidate {
                left: abstraction_ref(left),
                right: abstraction_ref(right),
                state_similarity: 0.0,
                behavior_similarity: Some(behavior.score),
                overall_similarity: behavior.score,
                substitutions: Vec::new(),
                shared_behavior: behavior.shared_labels,
                field_matches: Vec::new(),
                recommendation: Recommendation::ExtractGenericFunction,
            });
            continue;
        }
        let matching = matched_fields(left, right, &arena, &catalog, &mut cache);
        let largest = left.fields.len().max(right.fields.len());
        let state_similarity = if matching.pairs.is_empty() || largest == 0 {
            0.0
        } else {
            let type_similarity = matching
                .pairs
                .iter()
                .map(|pair| pair.match_similarity)
                .sum::<f64>()
                / count_f64(matching.pairs.len());
            let coverage = count_f64(matching.pairs.len()) / count_f64(largest);
            type_similarity * 0.5f64.mul_add(coverage, 0.5)
        };
        let state_candidate = !matching.pairs.is_empty()
            && matching.pairs.len() * 2 >= largest
            && state_similarity >= 0.60;
        if !state_candidate {
            continue;
        }
        let behavior = compare_behaviors(&left.behaviors, &right.behaviors);
        let weak_single_field =
            matching.pairs.len() == 1 && matching.pairs[0].name_similarity < 0.34;
        if weak_single_field
            && !behavior
                .as_ref()
                .is_some_and(|behavior| behavior.score >= 0.85)
        {
            continue;
        }
        let behavior_similarity = behavior.as_ref().map(|behavior| behavior.score);
        let overall_similarity = behavior_similarity.map_or(state_similarity, |behavior| {
            if state_similarity == 0.0 {
                behavior
            } else {
                0.40f64.mul_add(behavior, 0.60 * state_similarity)
            }
        });
        let substitutions = matching
            .pairs
            .iter()
            .flat_map(|pair| pair.substitutions.clone())
            .collect::<Vec<_>>();
        let field_matches = matching
            .pairs
            .iter()
            .map(|pair| FieldMatch {
                left: pair.left_name.clone(),
                right: pair.right_name.clone(),
                name_similarity: pair.name_similarity,
                type_similarity: pair.type_similarity,
                match_similarity: pair.match_similarity,
            })
            .collect();
        candidates.push(Candidate {
            left: abstraction_ref(left),
            right: abstraction_ref(right),
            state_similarity,
            behavior_similarity,
            overall_similarity,
            recommendation: recommendation(
                left,
                right,
                &substitutions,
                behavior_similarity,
                state_similarity,
                &matching,
            ),
            substitutions,
            shared_behavior: behavior.map_or_else(Vec::new, |behavior| behavior.shared_labels),
            field_matches,
        });
    }
    candidates.sort_by(|left, right| {
        right
            .overall_similarity
            .total_cmp(&left.overall_similarity)
            .then_with(|| left.left.path.cmp(&right.left.path))
            .then_with(|| left.left.line.cmp(&right.left.line))
            .then_with(|| left.right.path.cmp(&right.right.path))
            .then_with(|| left.right.line.cmp(&right.right.line))
    });
    Ok(AnalysisReport {
        candidates,
        schema_version: consts::BASELINE_SCHEMA_VERSION,
        scanned_files: sources.len(),
        abstractions: abstractions.len(),
        interned_shapes: arena.len(),
        cached_comparisons: cache.len(),
    })
}

fn candidate_pairs(
    abstractions: &[Abstraction],
    arena: &ShapeArena,
    catalog: &TypeCatalog,
) -> BTreeSet<(usize, usize)> {
    let mut buckets: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, abstraction) in abstractions.iter().enumerate() {
        match abstraction.kind {
            AbstractionKind::Function => {
                if let Some(behavior) = abstraction
                    .behaviors
                    .first()
                    .filter(|behavior| behavior.graph.is_substantive())
                {
                    buckets
                        .entry(format!("function:{}", behavior.graph.bucket_key()))
                        .or_default()
                        .push(index);
                }
            }
            AbstractionKind::Struct => {
                let mut keys = BTreeSet::new();
                for field in &abstraction.fields {
                    let shape = arena.bucket_key(field.shape, catalog);
                    keys.insert(format!("struct:{shape}"));
                }
                for key in keys {
                    buckets.entry(key).or_default().push(index);
                }
            }
        }
    }
    let mut pairs = BTreeSet::new();
    for indexes in buckets.values() {
        for (position, left) in indexes.iter().enumerate() {
            for right in &indexes[position + 1..] {
                let left_abstraction = &abstractions[*left];
                let right_abstraction = &abstractions[*right];
                if left_abstraction.kind == AbstractionKind::Struct {
                    let smallest = left_abstraction
                        .fields
                        .len()
                        .min(right_abstraction.fields.len());
                    let largest = left_abstraction
                        .fields
                        .len()
                        .max(right_abstraction.fields.len());
                    if smallest * 2 < largest {
                        continue;
                    }
                }
                pairs.insert((*left, *right));
            }
        }
    }
    pairs
}

#[derive(Debug)]
struct Abstraction {
    cfg: Predicate,
    primitive_fields: bool,
    kind: AbstractionKind,
    name: String,
    path: String,
    qualified_name: String,
    behaviors: Vec<Behavior>,
    fields: Vec<Field>,
    line: usize,
}

#[derive(Debug)]
struct Field {
    shape: ShapeId,
    name: String,
}

#[derive(Debug)]
struct Behavior {
    graph: BehaviorGraph,
    name: String,
}

#[derive(Debug)]
struct PendingImpl {
    module: String,
    owner: String,
    path: String,
    behaviors: Vec<Behavior>,
}

#[derive(Default)]
struct Collector {
    arena: ShapeArena,
    abstractions: Vec<Abstraction>,
    pending_impls: Vec<PendingImpl>,
}

impl Collector {
    fn collect_module(
        &mut self,
        items: &[Item],
        module: &str,
        path: &str,
        inherited: &Predicate,
        include_tests: bool,
    ) -> Result<()> {
        for item in items {
            if !include_tests && is_test_item(item)? {
                continue;
            }
            match item {
                Item::Struct(item) => {
                    let cfg = inherited
                        .clone()
                        .and(Predicate::from_attributes(&item.attrs)?);
                    if !include_tests && cfg.is_test_only() {
                        continue;
                    }
                    let generic_parameters = item
                        .generics
                        .params
                        .iter()
                        .filter_map(|parameter| match parameter {
                            GenericParam::Type(parameter) => Some(parameter.ident.to_string()),
                            GenericParam::Lifetime(_) | GenericParam::Const(_) => None,
                        })
                        .enumerate()
                        .map(|(index, name)| (name, index))
                        .collect::<BTreeMap<_, _>>();
                    let fields: Vec<Field> = match &item.fields {
                        Fields::Named(fields) => fields
                            .named
                            .iter()
                            .filter_map(|field| {
                                Some(Field {
                                    name: field.ident.as_ref()?.to_string(),
                                    shape: self.arena.intern(&field.ty, &generic_parameters),
                                })
                            })
                            .collect(),
                        Fields::Unnamed(fields) => fields
                            .unnamed
                            .iter()
                            .enumerate()
                            .map(|(index, field)| Field {
                                name: index.to_string(),
                                shape: self.arena.intern(&field.ty, &generic_parameters),
                            })
                            .collect(),
                        Fields::Unit => Vec::new(),
                    };
                    self.abstractions.push(Abstraction {
                        cfg,
                        primitive_fields: !fields.is_empty()
                            && fields.iter().all(|field| self.arena.is_scalar(field.shape)),
                        fields,
                        path: path.to_string(),
                        line: item.ident.span().start().line,
                        name: item.ident.to_string(),
                        qualified_name: qualify_source(path, module, &item.ident.to_string()),
                        kind: AbstractionKind::Struct,
                        behaviors: Vec::new(),
                    });
                }
                Item::Fn(item) => self.abstractions.push(Abstraction {
                    cfg: inherited
                        .clone()
                        .and(Predicate::from_attributes(&item.attrs)?),
                    primitive_fields: false,
                    path: path.to_string(),
                    line: item.sig.ident.span().start().line,
                    name: item.sig.ident.to_string(),
                    qualified_name: qualify_source(path, module, &item.sig.ident.to_string()),
                    kind: AbstractionKind::Function,
                    fields: Vec::new(),
                    behaviors: vec![Behavior {
                        name: item.sig.ident.to_string(),
                        graph: BehaviorGraph::from_function(&item.sig, &item.block),
                    }],
                }),
                Item::Mod(item_module) => {
                    if let Some((_, items)) = &item_module.content {
                        let cfg = inherited
                            .clone()
                            .and(Predicate::from_attributes(&item_module.attrs)?);
                        if !include_tests && cfg.is_test_only() {
                            continue;
                        }
                        self.collect_module(
                            items,
                            &qualify(module, &item_module.ident.to_string()),
                            path,
                            &cfg,
                            include_tests,
                        )?;
                    }
                }
                _ => {}
            }
        }
        for item in items {
            if !include_tests && is_test_item(item)? {
                continue;
            }
            let Item::Impl(item) = item else {
                continue;
            };
            let Some(owner) = crate::common::parse::self_ty_name(&item.self_ty) else {
                continue;
            };
            let owner_generics = item
                .generics
                .params
                .iter()
                .filter_map(|parameter| match parameter {
                    GenericParam::Type(parameter) => Some(parameter.ident.to_string()),
                    GenericParam::Lifetime(_) | GenericParam::Const(_) => None,
                })
                .enumerate()
                .map(|(index, name)| (name, index))
                .collect::<BTreeMap<_, _>>();
            let mut behaviors = Vec::new();
            for method in &item.items {
                let ImplItem::Fn(method) = method else {
                    continue;
                };
                behaviors.push(Behavior {
                    name: method.sig.ident.to_string(),
                    graph: BehaviorGraph::from_method(
                        &method.sig,
                        &method.block,
                        owner_generics.clone(),
                    ),
                });
            }
            self.pending_impls.push(PendingImpl {
                owner,
                behaviors,
                path: path.to_string(),
                module: module.to_string(),
            });
        }
        Ok(())
    }
}

fn attach_impls(abstractions: &mut [Abstraction], pending_impls: Vec<PendingImpl>) {
    for pending in pending_impls {
        let exact = qualify_source(&pending.path, &pending.module, &pending.owner);
        let target = abstractions
            .iter()
            .position(|abstraction| abstraction.qualified_name == exact)
            .or_else(|| {
                let candidates = abstractions
                    .iter()
                    .enumerate()
                    .filter(|(_, abstraction)| {
                        abstraction.kind == AbstractionKind::Struct
                            && abstraction.name == pending.owner
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                if candidates.len() == 1 {
                    candidates.first().copied()
                } else {
                    let owner_crate = crate_from_path(&pending.path);
                    let local = candidates
                        .into_iter()
                        .filter(|index| crate_from_path(&abstractions[*index].path) == owner_crate)
                        .collect::<Vec<_>>();
                    if local.len() == 1 {
                        local.first().copied()
                    } else {
                        None
                    }
                }
            });
        if let Some(target) = target {
            abstractions[target].behaviors.extend(pending.behaviors);
        }
    }
}

fn crate_from_path(path: &str) -> &str {
    if let Some((crate_root, _)) = path.split_once("/src/") {
        return crate_root;
    }
    let mut components = path.split('/');
    let first = components.next().unwrap_or(path);
    if first == "crates" {
        components.next().unwrap_or(path)
    } else {
        first
    }
}

fn is_test_item(item: &Item) -> Result<bool> {
    let attributes = match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => return Ok(false),
    };
    Ok(attributes
        .iter()
        .any(|attribute| attribute.path().is_ident("test"))
        || Predicate::from_attributes(attributes)?.is_test_only())
}

fn qualify(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}::{name}")
    }
}

fn qualify_source(path: &str, module: &str, name: &str) -> String {
    format!("{path}::{}", qualify(module, name))
}

fn abstraction_ref(abstraction: &Abstraction) -> AbstractionRef {
    AbstractionRef {
        path: abstraction.path.clone(),
        line: abstraction.line,
        name: abstraction.name.clone(),
        kind: abstraction.kind,
    }
}

fn recommendation(
    left: &Abstraction,
    right: &Abstraction,
    substitutions: &[TypeSubstitution],
    behavior_similarity: Option<f64>,
    state_similarity: f64,
    matching: &FieldMatching,
) -> Recommendation {
    if left.fields.len() != right.fields.len() {
        if behavior_similarity.is_some_and(|score| score >= 0.78) {
            Recommendation::ExtractComponent
        } else {
            Recommendation::ReviewPartialState
        }
    } else if !substitutions.is_empty()
        && state_similarity >= 0.80
        && semantically_aligned(matching, behavior_similarity)
    {
        Recommendation::GenericParameter
    } else if state_similarity >= 0.85 && semantically_aligned(matching, behavior_similarity) {
        Recommendation::Merge
    } else {
        Recommendation::ReviewStructuralOverlap
    }
}

fn semantically_aligned(matching: &FieldMatching, behavior_similarity: Option<f64>) -> bool {
    behavior_similarity.is_some_and(|score| score >= 0.85)
        || matching
            .pairs
            .iter()
            .all(|pair| pair.name_similarity >= 0.34)
}

struct BehaviorMatching {
    shared_labels: Vec<String>,
    score: f64,
}

fn compare_behaviors(left: &[Behavior], right: &[Behavior]) -> Option<BehaviorMatching> {
    struct PossiblePair {
        left: usize,
        right: usize,
        score: f64,
        shared_labels: Vec<String>,
    }

    if left.is_empty() || right.is_empty() {
        return None;
    }
    let mut possible = Vec::new();
    for (left_index, left_behavior) in left.iter().enumerate() {
        for (right_index, right_behavior) in right.iter().enumerate() {
            if !left_behavior.graph.may_match(&right_behavior.graph) {
                continue;
            }
            let comparison = left_behavior.graph.compare(&right_behavior.graph);
            if comparison.score >= 0.55 {
                let name_bonus = if left_behavior.name == right_behavior.name {
                    0.03
                } else {
                    0.0
                };
                possible.push(PossiblePair {
                    left: left_index,
                    right: right_index,
                    score: (comparison.score + name_bonus).min(1.0),
                    shared_labels: comparison.shared_labels,
                });
            }
        }
    }
    possible.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.left.cmp(&right.left))
            .then_with(|| left.right.cmp(&right.right))
    });
    let mut used_left = vec![false; left.len()];
    let mut used_right = vec![false; right.len()];
    let mut matched = Vec::new();
    let mut shared_labels = Vec::new();
    for possible in possible {
        if used_left[possible.left] || used_right[possible.right] {
            continue;
        }
        used_left[possible.left] = true;
        used_right[possible.right] = true;
        matched.push(possible.score);
        shared_labels.extend(possible.shared_labels);
    }
    if matched.is_empty() {
        return None;
    }
    shared_labels.sort();
    shared_labels.dedup();
    let average = matched.iter().sum::<f64>() / count_f64(matched.len());
    let coverage = count_f64(matched.len()) / count_f64(left.len().max(right.len()));
    Some(BehaviorMatching {
        shared_labels,
        score: average * 0.5f64.mul_add(coverage, 0.5),
    })
}

struct FieldMatching {
    pairs: Vec<FieldPair>,
}

struct FieldPair {
    left_name: String,
    right_name: String,
    substitutions: Vec<TypeSubstitution>,
    match_similarity: f64,
    name_similarity: f64,
    type_similarity: f64,
}

fn matched_fields(
    left: &Abstraction,
    right: &Abstraction,
    arena: &ShapeArena,
    catalog: &TypeCatalog,
    cache: &mut ComparisonCache,
) -> FieldMatching {
    struct PossiblePair {
        left: usize,
        right: usize,
        score: f64,
        pair: FieldPair,
    }

    let mut possible = Vec::new();
    for (left_index, left_field) in left.fields.iter().enumerate() {
        for (right_index, right_field) in right.fields.iter().enumerate() {
            let comparison = arena.compare(left_field.shape, right_field.shape, catalog, cache);
            let name_similarity = identifier_similarity(&left_field.name, &right_field.name);
            let score = 0.15f64.mul_add(name_similarity, 0.85 * comparison.score);
            if comparison.score >= 0.55 && score >= 0.60 {
                possible.push(PossiblePair {
                    left: left_index,
                    right: right_index,
                    score,
                    pair: FieldPair {
                        name_similarity,
                        left_name: left_field.name.clone(),
                        right_name: right_field.name.clone(),
                        type_similarity: comparison.score,
                        match_similarity: score,
                        substitutions: comparison.substitutions,
                    },
                });
            }
        }
    }
    possible.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.left.cmp(&right.left))
            .then_with(|| left.right.cmp(&right.right))
    });
    let mut used_left = vec![false; left.fields.len()];
    let mut used_right = vec![false; right.fields.len()];
    let mut pairs = Vec::new();
    for possible in possible {
        if used_left[possible.left] || used_right[possible.right] {
            continue;
        }
        used_left[possible.left] = true;
        used_right[possible.right] = true;
        pairs.push(possible.pair);
    }
    FieldMatching { pairs }
}

fn identifier_similarity(left: &str, right: &str) -> f64 {
    if left == right {
        return 1.0;
    }
    let left = identifier_tokens(left);
    let right = identifier_tokens(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let common = left.iter().filter(|token| right.contains(token)).count();
    2.0 * count_f64(common) / count_f64(left.len() + right.len())
}

fn count_f64(value: usize) -> f64 {
    u32::try_from(value).map_or_else(|_| f64::from(u32::MAX), f64::from)
}

fn identifier_tokens(identifier: &str) -> Vec<String> {
    identifier
        .split('_')
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn twins(sources: &[(&str, &str)]) -> AnalysisReport {
        let sources = sources
            .iter()
            .map(|(path, source)| (path.to_string(), source.to_string()))
            .collect::<Vec<_>>();
        analyze_sources(&sources, &[], &SimilarityConfig::default(), false).expect("analysis")
    }

    const NEWTYPE: &str = r#"
        struct Count(u64);
        impl Count { fn next(&self) -> u64 { self.0 + 1 } }
    "#;

    #[test]
    fn cfg_alternatives_are_not_twins_but_overlapping_modules_are() {
        let roots = r#"
            #[cfg(feature = "flash")]
            mod enabled { mod lock; }
            #[cfg(all(feature = "flash", not(feature = "loom")))]
            mod backend;
            #[cfg(not(feature = "flash"))]
            mod system;
        "#;
        let report = twins(&[
            ("crates/sample/src/lib.rs", roots),
            ("crates/sample/src/enabled/lock.rs", NEWTYPE),
            ("crates/sample/src/backend.rs", NEWTYPE),
            ("crates/sample/src/system.rs", NEWTYPE),
        ]);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].recommendation, Recommendation::Merge);
        assert!(
            report.candidates[0]
                .behavior_similarity
                .is_some_and(|score| score >= 0.9)
        );

        let report = twins(&[(
            "src/lib.rs",
            r#"
            #[cfg(any(target_os = "ios", target_os = "android"))]
            struct Mobile(u64);
            #[cfg(all(not(target_os = "ios"), target_os = "android"))]
            struct Android(u64);
            #[cfg(target_os = "linux")]
            struct Linux(u64);
        "#,
        )]);
        assert_eq!(report.candidates.len(), 1);

        let report = twins(&[
            (
                "src/lib.rs",
                "#[cfg(unix)] mod nested; #[cfg(not(unix))] mod other;",
            ),
            ("src/nested.rs", "mod main;"),
            ("src/nested/main.rs", NEWTYPE),
            ("src/other.rs", NEWTYPE),
        ]);
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn test_paths_and_out_of_line_test_modules_are_not_product_twins() {
        for path in [
            "tests.rs",
            "count_tests.rs",
            "count_test.rs",
            "tests/count.rs",
        ] {
            let report = twins(&[
                ("crates/sample/src/lib.rs", NEWTYPE),
                (&format!("crates/sample/src/{path}"), NEWTYPE),
            ]);
            assert!(report.candidates.is_empty(), "test path: {path}");
        }
        let report = twins(&[
            (
                "crates/sample/src/lib.rs",
                "mod live; #[cfg(test)] mod fixtures;",
            ),
            ("crates/sample/src/live.rs", NEWTYPE),
            ("crates/sample/src/fixtures.rs", "mod nested;"),
            ("crates/sample/src/fixtures/nested.rs", NEWTYPE),
        ]);
        assert_eq!(report.abstractions, 1);
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn primitive_fields_are_dropped_only_across_unrelated_crates() {
        for scalar in ["u64", "i32", "f32", "bool", "char", "std::num::NonZeroU64"] {
            let source = r#"
                struct Count { first: u64, second: u64 }
                impl Count { fn text(&self) -> String { self.first.to_string() } }
            "#
            .replace("u64", scalar);
            let report = twins(&[
                ("crates/first/src/lib.rs", &source),
                ("crates/second/src/lib.rs", &source),
            ]);
            assert!(report.candidates.is_empty(), "cross-crate scalar: {scalar}");
            let report = twins(&[
                ("crates/first/src/left.rs", &source),
                ("crates/first/src/right.rs", &source),
            ]);
            assert_eq!(report.candidates.len(), 1, "same-crate scalar: {scalar}");
        }
        let report = twins(&[
            ("crates/first/src/lib.rs", "struct Count(Vec<u64>);"),
            ("crates/second/src/lib.rs", "struct Count(Vec<u64>);"),
        ]);
        assert_eq!(report.candidates.len(), 1);
    }

    #[test]
    fn primitive_fields_remain_twins_across_dependent_crates() {
        let sources = [
            (
                "crates/first/src/lib.rs".to_owned(),
                "struct Pair { first: usize, second: usize }".to_owned(),
            ),
            (
                "crates/second/src/lib.rs".to_owned(),
                "struct Pair { first: usize, second: usize }".to_owned(),
            ),
        ];
        for (owner, dependency) in [
            ("crates/first", "crates/second"),
            ("crates/second", "crates/first"),
        ] {
            let mut config = SimilarityConfig::default();
            config.workspace_dependencies.insert(
                owner.to_owned(),
                [dependency.to_owned()].into_iter().collect(),
            );
            let report = analyze_sources(&sources, &[], &config, false).expect("analysis");
            assert_eq!(report.candidates.len(), 1);
        }
    }

    #[test]
    fn non_primitive_field_keeps_unrelated_crates_in_scope() {
        let report = twins(&[
            (
                "crates/first/src/lib.rs",
                "struct Pair { first: usize, second: Vec<u64> }",
            ),
            (
                "crates/second/src/lib.rs",
                "struct Pair { first: usize, second: Vec<u64> }",
            ),
        ]);
        assert_eq!(report.candidates.len(), 1);
    }

    #[test]
    fn same_crate_newtypes_gate_without_field_name_agreement() {
        let report = twins(&[(
            "crates/sample/src/lib.rs",
            r#"
            struct FrameCount(u64);
            impl FrameCount { fn next(&self) -> u64 { self.0 + 1 } }
            struct SampleCount(u64);
            impl SampleCount { fn next(&self) -> u64 { self.0 + 1 } }
            struct Frames { frames: u64 }
            impl Frames { fn next(&self) -> u64 { self.frames + 1 } }
            struct Samples { samples: u64 }
            impl Samples { fn next(&self) -> u64 { self.samples + 1 } }
        "#,
        )]);
        assert_eq!(
            report
                .candidates
                .iter()
                .filter(|candidate| candidate.is_twin(0.9))
                .count(),
            6
        );
        assert!(!report.candidates[0].is_twin(1.01));
    }

    #[test]
    fn function_pairs_never_gate() {
        let report = twins(&[(
            "src/lib.rs",
            r#"
            fn left<T: Clone>(values: &[T]) -> Option<T> {
                let value = values.first()?;
                Some(value.clone())
            }
            fn right<U: Clone>(values: &[U]) -> Option<U> {
                let value = values.first()?;
                Some(value.clone())
            }
        "#,
        )]);
        assert_eq!(report.candidates.len(), 1);
        assert!(
            report
                .candidates
                .iter()
                .all(|candidate| !candidate.is_twin(0.9))
        );
    }

    #[test]
    fn nested_derived_partial_shapes_are_composition_candidates() {
        let source = r#"
            #[derive(Clone, Debug)]
            struct Foo<T> {
                a: Vec<Option<T>>,
                b: Daz,
            }

            #[derive(Clone)]
            struct Bar<U> {
                a: Vec<Option<U>>,
            }
        "#;

        let report = analyze_source("fixture.rs", source).expect("analyze source");

        assert!(report.candidates.iter().any(|candidate| {
            candidate.left.name == "Foo"
                && candidate.right.name == "Bar"
                && candidate.recommendation == Recommendation::ReviewPartialState
        }));
    }

    #[test]
    fn configured_container_relation_has_score_and_caveats() {
        let source = r#"
            struct Foo {
                values: List<Option<Baz>>,
            }

            struct Bar {
                values: Vec<Option<Baz>>,
            }
        "#;
        let config = toml::from_str(
            r#"
                [gate]
                min_behavior = 0.9

                [[types.relations]]
                left = "List"
                right = "Vec"
                similarity = 0.84
                substitution = "conditional"
                caveats = ["allocation and indexing semantics"]
            "#,
        )
        .expect("similarity config");

        let report =
            analyze_source_with_config("fixture.rs", source, &config).expect("analyze source");
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| candidate.left.name == "Foo" && candidate.right.name == "Bar")
            .expect("container relation candidate");

        assert!(candidate.state_similarity > 0.84);
        assert!(
            candidate
                .field_matches
                .iter()
                .any(|field| (field.type_similarity - 0.84).abs() < 0.001)
        );
        assert!(candidate.substitutions.iter().any(|substitution| {
            substitution.left == "List"
                && substitution.right == "Vec"
                && substitution
                    .caveats
                    .contains(&"allocation and indexing semantics".to_string())
        }));
    }

    #[test]
    fn generic_functions_match_by_normalized_behavior() {
        let source = r#"
            fn collect_first<T: Clone>(items: &[T]) -> Option<T> {
                let selected = items.first()?;
                Some(selected.clone())
            }

            fn head<U: Clone>(values: &[U]) -> Option<U> {
                let value = values.first()?;
                Some(value.clone())
            }
        "#;

        let report = analyze_source("fixture.rs", source).expect("analyze source");
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| {
                candidate.left.name == "collect_first"
                    && candidate.right.name == "head"
                    && candidate.left.kind == AbstractionKind::Function
                    && candidate.right.kind == AbstractionKind::Function
            })
            .expect("generic function candidate");

        assert!(
            candidate
                .behavior_similarity
                .is_some_and(|score| score >= 0.85)
        );
        assert_eq!(
            candidate.recommendation,
            Recommendation::ExtractGenericFunction
        );
    }

    #[test]
    fn partial_state_and_similar_impl_recommend_composition() {
        let source = r#"
            struct Foo<T> {
                values: Vec<T>,
                metadata: Daz,
            }

            impl<T> Foo<T> {
                fn append(&mut self, value: T) {
                    self.values.push(value);
                }
            }

            struct Bar<U> {
                values: VecDeque<U>,
            }

            impl<U> Bar<U> {
                fn add(&mut self, item: U) {
                    self.values.push_back(item);
                }
            }
        "#;

        let report = analyze_source("fixture.rs", source).expect("analyze source");
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| candidate.left.name == "Foo" && candidate.right.name == "Bar")
            .expect("partial abstraction candidate");

        assert!(
            candidate
                .behavior_similarity
                .is_some_and(|score| score >= 0.85)
        );
        assert_eq!(candidate.recommendation, Recommendation::ExtractComponent);
    }

    #[test]
    fn cfg_test_items_are_excluded_from_native_analysis() {
        let source = r#"
            struct Production { value: usize }

            #[cfg(test)]
            mod tests {
                struct FixtureA { value: usize }
                struct FixtureB { value: usize }
            }
        "#;

        let report = analyze_source("src/lib.rs", source).expect("analyze source");

        assert!(report.candidates.iter().all(|candidate| {
            !candidate.left.name.starts_with("Fixture")
                && !candidate.right.name.starts_with("Fixture")
        }));
    }

    #[test]
    fn unrelated_numeric_records_are_not_twins_without_matching_behavior() {
        let source = r#"
            struct Position {
                end_position_ns: usize,
                frame_offset: usize,
                frames: usize,
            }

            struct SeekError {
                current_pos: usize,
                len: usize,
                new_pos: usize,
            }
        "#;

        let report = analyze_source("src/lib.rs", source).expect("analyze source");

        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].recommendation,
            Recommendation::ReviewStructuralOverlap
        );
        assert!(!report.candidates[0].is_twin(0.9));
    }

    #[test]
    fn unrelated_generic_wrappers_need_behavior_or_field_semantics() {
        let source = r#"
            struct Writer<W> { inner: W }
            struct Unit<B>(B);
        "#;

        let report = analyze_source("src/lib.rs", source).expect("analyze source");

        assert!(report.candidates.is_empty());
    }

    #[test]
    fn optional_numeric_fields_are_not_twins_without_matching_behavior() {
        let source = r#"
            struct Clock {
                last_beat: Option<u64>,
                published_track: Option<usize>,
            }

            struct Capacity {
                max_bytes: Option<u64>,
                max_assets: Option<usize>,
            }
        "#;

        let report = analyze_source("src/lib.rs", source).expect("analyze source");

        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].recommendation,
            Recommendation::ReviewStructuralOverlap
        );
        assert!(!report.candidates[0].is_twin(0.9));
    }

    #[test]
    fn trivial_constant_functions_are_not_refactoring_candidates() {
        let source = r#"
            fn default_capacity() -> usize { 8 }
            fn default_retries() -> usize { 3 }
        "#;

        let report = analyze_source("src/lib.rs", source).expect("analyze source");

        assert!(report.candidates.is_empty());
    }

    #[test]
    fn impls_in_separate_files_attach_to_unique_workspace_types() {
        let sources = vec![
            (
                "src/types.rs".to_string(),
                r#"
                    struct Foo<T> {
                        values: Vec<T>,
                        metadata: Daz,
                    }
                    struct Bar<U> {
                        values: VecDeque<U>,
                    }
                "#
                .to_string(),
            ),
            (
                "src/foo_impl.rs".to_string(),
                r#"
                    impl<T> Foo<T> {
                        fn append(&mut self, value: T) {
                            self.values.push(value);
                        }
                    }
                "#
                .to_string(),
            ),
            (
                "src/bar_impl.rs".to_string(),
                r#"
                    impl<U> Bar<U> {
                        fn add(&mut self, item: U) {
                            self.values.push_back(item);
                        }
                    }
                "#
                .to_string(),
            ),
        ];

        let report =
            analyze_sources(&sources, &[], &SimilarityConfig::default(), false).expect("analysis");
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| candidate.left.name == "Foo" && candidate.right.name == "Bar")
            .expect("cross-file impl candidate");

        assert!(
            candidate
                .behavior_similarity
                .is_some_and(|score| score >= 0.85)
        );
        assert_eq!(candidate.recommendation, Recommendation::ExtractComponent);
    }
}
