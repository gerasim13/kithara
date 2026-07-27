use syn::{Expr, Type, visit::Visit};

use super::graph::{Edge, EdgeKind, Evidence, EvidenceGraph, Node, NodeId, NodeKind};

pub(super) fn collect(
    package: &str,
    target: &str,
    module: String,
    relative: &str,
    file: &syn::File,
    graph: &mut EvidenceGraph,
) {
    let mut visitor = OwnershipVisitor {
        package,
        target,
        module,
        inline_modules: Vec::new(),
        relative,
        graph,
    };
    visitor.visit_file(file);
}

struct OwnershipVisitor<'a> {
    package: &'a str,
    target: &'a str,
    module: String,
    inline_modules: Vec<String>,
    relative: &'a str,
    graph: &'a mut EvidenceGraph,
}

impl OwnershipVisitor<'_> {
    fn current_module(&self) -> String {
        if self.inline_modules.is_empty() {
            self.module.clone()
        } else if self.module == "crate" {
            self.inline_modules.join("::")
        } else {
            format!("{}::{}", self.module, self.inline_modules.join("::"))
        }
    }

    fn add_site(
        &mut self,
        line: usize,
        column: usize,
        tag: &str,
        label: String,
        edge_kind: EdgeKind,
        resource: &str,
    ) {
        let module = self.current_module();
        let site_id = NodeId::site(
            self.package,
            self.target,
            &module,
            &format!("{tag}@{line}:{column}"),
        );
        let resource_id = NodeId::resource(self.package, self.target, resource);
        let origin = format!("{}:{line}:{column}", self.relative);
        self.graph.merge_node(
            Node::new(
                site_id.clone(),
                NodeKind::OwnershipSite,
                label,
                Evidence::static_fact(origin.clone()),
            )
            .at(self.relative, line),
        );
        let resource_evidence = if resource == "?" {
            Evidence::unresolved(origin.clone())
        } else {
            Evidence::static_fact(origin.clone())
        };
        self.graph.merge_node(Node::new(
            resource_id.clone(),
            NodeKind::Resource,
            format!("Arc<{resource}>"),
            resource_evidence,
        ));
        let edge_evidence = if resource == "?" {
            Evidence::unresolved(origin)
        } else {
            Evidence::static_fact(origin)
        };
        self.graph
            .merge_edge(Edge::new(site_id, resource_id, edge_kind, edge_evidence));
    }
}

impl<'ast> Visit<'ast> for OwnershipVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.content.is_some() {
            self.inline_modules.push(item.ident.to_string());
            syn::visit::visit_item_mod(self, item);
            self.inline_modules.pop();
        }
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        use syn::spanned::Spanned as _;

        for field in &item.fields {
            let Some(resource) = arc_inner(&field.ty) else {
                continue;
            };
            let start = field.span().start();
            let field_name = field
                .ident
                .as_ref()
                .map_or_else(|| "?".to_string(), ToString::to_string);
            self.add_site(
                start.line,
                start.column,
                "arc-field",
                format!("{}::{field_name}: Arc<{resource}>", item.ident),
                EdgeKind::Stores,
                &resource,
            );
        }
        syn::visit::visit_item_struct(self, item);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        use syn::spanned::Spanned as _;

        if let Expr::Path(path) = call.func.as_ref() {
            let segments = path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            if let [.., owner, method] = segments.as_slice()
                && owner == "Arc"
            {
                let start = call.func.span().start();
                match method.as_str() {
                    "new" | "new_cyclic" | "from" => self.add_site(
                        start.line,
                        start.column,
                        "arc-create",
                        format!("Arc::{method}(...)"),
                        EdgeKind::Constructs,
                        "?",
                    ),
                    "clone" => self.add_site(
                        start.line,
                        start.column,
                        "arc-clone",
                        "Arc::clone(...)".to_string(),
                        EdgeKind::Clones,
                        "?",
                    ),
                    _ => {}
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn arc_inner(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Arc" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.first().map(render_generic_arg)
}

fn render_generic_arg(argument: &syn::GenericArgument) -> String {
    match argument {
        syn::GenericArgument::Type(ty) => render_type(ty),
        _ => "?".to_string(),
    }
}

fn render_type(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .iter()
            .map(|segment| {
                let mut rendered = segment.ident.to_string();
                if let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments {
                    let inner = arguments
                        .args
                        .iter()
                        .map(render_generic_arg)
                        .collect::<Vec<_>>();
                    rendered.push('<');
                    rendered.push_str(&inner.join(","));
                    rendered.push('>');
                }
                rendered
            })
            .collect::<Vec<_>>()
            .join("::"),
        Type::Reference(reference) => format!("&{}", render_type(&reference.elem)),
        Type::Tuple(tuple) => {
            let inner = tuple.elems.iter().map(render_type).collect::<Vec<_>>();
            format!("({})", inner.join(","))
        }
        _ => "?".to_string(),
    }
}
