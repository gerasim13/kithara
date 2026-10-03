use std::collections::BTreeMap;

use crate::{
    error::UiDocError,
    ids::SourceUri,
    module::{ControlNode, ModuleDoc, parse_module},
    source::{Limits, LoadedSource, ModuleSource, SourceResolver},
    validate,
};

#[derive(Debug, Default)]
pub(crate) struct ModuleSet {
    pub(crate) defs: BTreeMap<SourceUri, ModuleDoc>,
    /// Every shader a module declares, under the document that declared it.
    /// Nested rather than keyed by a pair so that a lookup borrows both halves
    /// of the key instead of building one.
    shaders: BTreeMap<SourceUri, BTreeMap<String, LoadedSource>>,
}

impl ModuleSet {
    pub(crate) fn shader(&self, origin: &SourceUri, source: &str) -> Option<&LoadedSource> {
        self.shaders.get(origin)?.get(source)
    }
}

pub(crate) fn load_module_graph(
    resolver: &dyn SourceResolver,
    base: Option<&SourceUri>,
    rel: &str,
    limits: &Limits,
) -> Result<(SourceUri, ModuleSet), UiDocError> {
    let mut set = ModuleSet::default();
    let mut stack = Vec::new();
    let uri = load_rec(resolver, base, rel, limits, &mut set, &mut stack, 0)?;
    Ok((uri, set))
}

fn load_rec(
    resolver: &dyn SourceResolver,
    base: Option<&SourceUri>,
    rel: &str,
    limits: &Limits,
    set: &mut ModuleSet,
    stack: &mut Vec<SourceUri>,
    depth: usize,
) -> Result<SourceUri, UiDocError> {
    let loaded = resolver.module(base, rel)?;
    if stack.contains(&loaded.uri) {
        let mut chain = stack.clone();
        chain.push(loaded.uri);
        return Err(UiDocError::IncludeCycle { chain });
    }
    if depth >= limits.max_depth {
        return Err(UiDocError::DepthExceeded {
            depth,
            origin: loaded.uri,
            max: limits.max_depth,
        });
    }
    if set.defs.contains_key(&loaded.uri) {
        return Ok(loaded.uri);
    }
    let doc = match loaded.source {
        ModuleSource::Text(text) => {
            if text.len() > limits.max_bytes {
                return Err(UiDocError::TooLarge {
                    bytes: text.len(),
                    origin: loaded.uri,
                    max: limits.max_bytes,
                });
            }
            parse_module(&text, &loaded.uri)?
        }
        ModuleSource::Document(doc) => {
            doc.check(&loaded.uri)?;
            *doc
        }
    };
    validate::check_module_id(&doc, &loaded.uri)?;
    validate::check_module_node_ids(&doc, &loaded.uri)?;
    stack.push(loaded.uri.clone());
    walk_includes(resolver, &loaded.uri, &doc.root, limits, set, stack, depth)?;
    let popped = stack.pop();
    debug_assert_eq!(popped.as_ref(), Some(&loaded.uri));
    set.defs.insert(loaded.uri.clone(), doc);
    Ok(loaded.uri)
}

fn load_source(
    resolver: &dyn SourceResolver,
    base: Option<&SourceUri>,
    rel: &str,
    limits: &Limits,
) -> Result<LoadedSource, UiDocError> {
    let loaded = resolver.load(base, rel)?;
    let bytes = loaded.text.len();
    if bytes > limits.max_bytes {
        return Err(UiDocError::TooLarge {
            bytes,
            origin: loaded.uri,
            max: limits.max_bytes,
        });
    }
    Ok(loaded)
}

fn walk_includes(
    resolver: &dyn SourceResolver,
    origin: &SourceUri,
    node: &ControlNode,
    limits: &Limits,
    set: &mut ModuleSet,
    stack: &mut Vec<SourceUri>,
    depth: usize,
) -> Result<(), UiDocError> {
    match node {
        ControlNode::Row { children, .. }
        | ControlNode::Column { children, .. }
        | ControlNode::Stage { children, .. } => {
            for child in children {
                walk_includes(resolver, origin, child, limits, set, stack, depth)?;
            }
            Ok(())
        }
        ControlNode::Slot { default, .. } => {
            for child in default {
                walk_includes(resolver, origin, child, limits, set, stack, depth)?;
            }
            Ok(())
        }
        ControlNode::Adaptive { base, steps, .. } => {
            walk_includes(resolver, origin, base, limits, set, stack, depth)?;
            for step in steps {
                walk_includes(resolver, origin, &step.node, limits, set, stack, depth)?;
            }
            Ok(())
        }
        ControlNode::Object { child, .. }
        | ControlNode::Optional { child, .. }
        | ControlNode::Placed { child, .. }
        | ControlNode::Pressable { child, .. }
        | ControlNode::Reveal { child, .. }
        | ControlNode::Scroll { child, .. } => {
            walk_includes(resolver, origin, child, limits, set, stack, depth)
        }
        ControlNode::Popover {
            anchor, content, ..
        } => {
            walk_includes(resolver, origin, anchor, limits, set, stack, depth)?;
            walk_includes(resolver, origin, content, limits, set, stack, depth)
        }
        ControlNode::Include { source, .. } => {
            load_rec(
                resolver,
                Some(origin),
                source,
                limits,
                set,
                stack,
                depth + 1,
            )?;
            Ok(())
        }
        ControlNode::Shader { source, .. } => {
            if set.shader(origin, source).is_none() {
                let loaded = load_source(resolver, Some(origin), source, limits)?;
                set.shaders
                    .entry(origin.clone())
                    .or_default()
                    .insert(source.clone(), loaded);
            }
            Ok(())
        }
        ControlNode::DeckSummary { .. }
        | ControlNode::Brand { .. }
        | ControlNode::Spacer { .. }
        | ControlNode::Divider { .. }
        | ControlNode::PresetSelector { .. }
        | ControlNode::SettingsButton { .. }
        | ControlNode::WindowDrag { .. }
        | ControlNode::TitleBar { .. }
        | ControlNode::WindowControls { .. }
        | ControlNode::Text { .. }
        | ControlNode::Glyph { .. }
        | ControlNode::NavItem { .. }
        | ControlNode::TabLarge { .. }
        | ControlNode::Button { .. }
        | ControlNode::Bpm { .. }
        | ControlNode::Time { .. }
        | ControlNode::Scalar { .. }
        | ControlNode::Crossfader { .. }
        | ControlNode::Fader { .. }
        | ControlNode::Wave { .. }
        | ControlNode::Vis { .. }
        | ControlNode::Lottie { .. }
        | ControlNode::Sprite { .. }
        | ControlNode::Custom { .. }
        | ControlNode::PortalMap { .. }
        | ControlNode::Range { .. }
        | ControlNode::Table { .. }
        | ControlNode::Tree { .. }
        | ControlNode::ContextBar { .. }
        | ControlNode::Toggle { .. }
        | ControlNode::Checkbox { .. }
        | ControlNode::Segmented { .. }
        | ControlNode::Select { .. }
        | ControlNode::StatusDot { .. }
        | ControlNode::Swatch { .. }
        | ControlNode::Cell { .. }
        | ControlNode::Readout { .. }
        | ControlNode::Chip { .. }
        | ControlNode::Knob { .. }
        | ControlNode::VuStereo { .. }
        | ControlNode::VuVertical { .. }
        | ControlNode::Meter { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::source::{Limits, MemResolver};

    fn module(id: &str, body: &str) -> String {
        format!(r#"(schema: "kithara.module", version: 1, id: "{id}", root: {body})"#)
    }

    #[kithara::test]
    fn ready_modules_share_envelope_validation_with_text() {
        for (schema, version) in [
            ("unknown", 1),
            ("kithara.module", 0),
            ("kithara.module", 2),
            ("kithara.layout", 1),
        ] {
            let origin = SourceUri("ready.kmodule.ron".to_owned());
            let mut doc = ModuleDoc::new(
                crate::ids::DocId("ready".to_owned()),
                ControlNode::Spacer {
                    id: crate::ids::NodeId("body".to_owned()),
                    size: None,
                    read: None,
                    write: None,
                },
            );
            doc.schema = schema.to_owned();
            doc.version = version;
            let text = format!(
                r#"(schema: "{schema}", version: {version}, id: "ready", root: Spacer(id: "body"))"#
            );
            let expected = parse_module(&text, &origin).unwrap_err();
            let mut resolver = MemResolver::default();
            resolver.insert_module(&origin.0, doc);
            let actual =
                load_module_graph(&resolver, None, &origin.0, &Limits::default()).unwrap_err();
            assert_eq!(
                std::mem::discriminant(&actual),
                std::mem::discriminant(&expected)
            );
            assert_eq!(actual.to_string(), expected.to_string());
        }
    }

    #[kithara::test]
    fn ready_modules_follow_relative_includes_through_package_overlays() {
        let mut ready = MemResolver::default();
        ready.insert_module(
            "sub/a.kmodule.ron",
            ModuleDoc::new(
                crate::ids::DocId("a".to_owned()),
                ControlNode::Include {
                    id: crate::ids::NodeId("b".to_owned()),
                    source: "b.kmodule.ron".to_owned(),
                    with: BTreeMap::new(),
                },
            ),
        );
        let mut text = MemResolver::default();
        text.insert("sub/b.kmodule.ron", &module("b", r#"Spacer(id: "body")"#));
        let resolver = crate::source::OverlayResolver::new(ready, text);
        let (_, set) =
            load_module_graph(&resolver, None, "sub/a.kmodule.ron", &Limits::default()).unwrap();
        assert_eq!(set.defs.len(), 2);
        assert!(
            set.defs
                .contains_key(&SourceUri("sub/b.kmodule.ron".to_owned()))
        );
    }

    #[kithara::test]
    fn ready_modules_share_include_cycle_detection_with_text() {
        let mut resolver = MemResolver::default();
        resolver.insert_module(
            "a.kmodule.ron",
            ModuleDoc::new(
                crate::ids::DocId("a".to_owned()),
                ControlNode::Include {
                    id: crate::ids::NodeId("b".to_owned()),
                    source: "b.kmodule.ron".to_owned(),
                    with: BTreeMap::new(),
                },
            ),
        );
        resolver.insert(
            "b.kmodule.ron",
            &module("b", r#"Include(id: "a", source: "a.kmodule.ron")"#),
        );
        let error =
            load_module_graph(&resolver, None, "a.kmodule.ron", &Limits::default()).unwrap_err();
        assert!(
            matches!(error, UiDocError::IncludeCycle { chain } if chain == ["a.kmodule.ron", "b.kmodule.ron", "a.kmodule.ron"].map(|uri| SourceUri(uri.to_owned())))
        );
    }

    #[kithara::test]
    fn loads_nested_includes_two_levels_deep() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "a.kmodule.ron",
            &module(
                "a",
                r#"Row(children: [Include(id: "b", source: "sub/b.kmodule.ron")])"#,
            ),
        );
        resolver.insert(
            "sub/b.kmodule.ron",
            &module(
                "b",
                r#"Row(children: [Include(id: "c", source: "c.kmodule.ron")])"#,
            ),
        );
        resolver.insert("sub/c.kmodule.ron", &module("c", r#"Text(id: "x")"#));

        let (uri, set) =
            load_module_graph(&resolver, None, "a.kmodule.ron", &Limits::default()).unwrap();
        assert_eq!(uri.0, "a.kmodule.ron");
        assert_eq!(set.defs.len(), 3);
    }

    #[kithara::test]
    fn include_cycle_reports_full_chain() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "a.kmodule.ron",
            &module(
                "a",
                r#"Row(children: [Include(id: "b", source: "b.kmodule.ron")])"#,
            ),
        );
        resolver.insert(
            "b.kmodule.ron",
            &module(
                "b",
                r#"Row(children: [Include(id: "a", source: "a.kmodule.ron")])"#,
            ),
        );

        let error =
            load_module_graph(&resolver, None, "a.kmodule.ron", &Limits::default()).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("a.kmodule.ron -> b.kmodule.ron -> a.kmodule.ron"),
            "{message}"
        );
    }

    #[kithara::test]
    fn depth_limit_is_enforced() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "a.kmodule.ron",
            &module(
                "a",
                r#"Row(children: [Include(id: "b", source: "b.kmodule.ron")])"#,
            ),
        );
        resolver.insert("b.kmodule.ron", &module("b", r#"Text(id: "x")"#));

        let limits = Limits {
            max_depth: 1,
            ..Limits::default()
        };
        let error = load_module_graph(&resolver, None, "a.kmodule.ron", &limits).unwrap_err();
        assert!(matches!(
            error,
            UiDocError::DepthExceeded {
                depth: 1,
                max: 1,
                ..
            }
        ));
    }

    #[kithara::test]
    fn shared_include_is_loaded_once_not_a_cycle() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "a.kmodule.ron",
            &module(
                "a",
                r#"Row(children: [
                    Include(id: "left", source: "shared.kmodule.ron"),
                    Include(id: "right", source: "shared.kmodule.ron"),
                ])"#,
            ),
        );
        resolver.insert("shared.kmodule.ron", &module("shared", r#"Text(id: "x")"#));

        load_module_graph(&resolver, None, "a.kmodule.ron", &Limits::default()).unwrap();
    }

    #[kithara::test]
    fn oversized_included_source_is_rejected() {
        let entry = module("a", r#"Include(id: "b", source: "b.kmodule.ron")"#);
        let child = module(
            "b",
            &format!(r#"Chip(id: "text", label: "{}")"#, "x".repeat(256)),
        );
        assert!(child.len() > entry.len());
        let mut resolver = MemResolver::default();
        resolver.insert("a.kmodule.ron", &entry);
        resolver.insert("b.kmodule.ron", &child);
        let limits = Limits {
            max_bytes: entry.len(),
            ..Limits::default()
        };

        let error = load_module_graph(&resolver, None, "a.kmodule.ron", &limits).unwrap_err();
        assert!(matches!(
            error,
            UiDocError::TooLarge { origin, bytes, max }
                if origin.0 == "b.kmodule.ron" && bytes == child.len() && max == entry.len()
        ));
    }
}
