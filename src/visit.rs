// Copyright 2021, 2022 Martin Pool

//! Visit the abstract syntax tree and discover things to mutate.
//!
//! Knowledge of the syn API is localized here.

use std::sync::Arc;

use quote::ToTokens;
use syn::visit::Visit;
use syn::Attribute;
use syn::ItemFn;

use crate::*;

/// Find all possible mutants in a source file.
pub fn discover_mutants(source_file: Arc<SourceFile>) -> Result<Vec<Mutant>> {
    let syn_file = syn::parse_str::<syn::File>(&source_file.code)?;
    let mut visitor = DiscoveryVisitor {
        source_file,
        mutants: Vec::new(),
        namespace_stack: Vec::new(),
    };
    visitor.visit_file(&syn_file);
    Ok(visitor.mutants)
}

/// `syn` visitor that recursively traverses the syntax tree, accumulating places
/// that could be mutated.
struct DiscoveryVisitor {
    /// All the mutants generated by visiting the file.
    mutants: Vec<Mutant>,

    /// The file being visited.
    source_file: Arc<SourceFile>,

    /// The stack of namespaces we're currently inside.
    namespace_stack: Vec<String>,
}

impl DiscoveryVisitor {
    fn collect_fn_mutants(&mut self, return_type: &syn::ReturnType, span: &proc_macro2::Span) {
        let full_function_name = Arc::new(self.namespace_stack.join("::"));
        let return_type_str = Arc::new(return_type_to_string(return_type));
        for op in ops_for_return_type(return_type) {
            self.mutants.push(Mutant::new(
                self.source_file.clone(),
                op,
                full_function_name.clone(),
                return_type_str.clone(),
                span.into(),
            ))
        }
    }

    /// Call a function with a namespace pushed onto the stack.
    ///
    /// This is used when recursively descending into a namespace.
    fn in_namespace<F, T>(&mut self, name: &str, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        self.namespace_stack.push(name.to_owned());
        let r = f(self);
        assert_eq!(self.namespace_stack.pop().unwrap(), name);
        r
    }
}

impl<'ast> Visit<'ast> for DiscoveryVisitor {
    fn visit_item_fn(&mut self, i: &'ast ItemFn) {
        // TODO: Filter out more inapplicable fns.
        if attrs_excluded(&i.attrs) || block_is_empty(&i.block) {
            return; // don't look inside it either
        }
        let function_name = remove_excess_spaces(&i.sig.ident.to_token_stream().to_string());
        self.in_namespace(&function_name, |self_| {
            self_.collect_fn_mutants(&i.sig.output, &i.block.brace_token.span);
            syn::visit::visit_item_fn(self_, i);
        });
    }

    /// Visit `fn foo()` within an `impl`.
    fn visit_impl_item_method(&mut self, i: &'ast syn::ImplItemMethod) {
        // Don't look inside constructors (called "new") because there's often no good
        // alternative.
        if attrs_excluded(&i.attrs) || i.sig.ident == "new" || block_is_empty(&i.block) {
            return;
        }
        let function_name = remove_excess_spaces(&i.sig.ident.to_token_stream().to_string());
        self.in_namespace(&function_name, |self_| {
            self_.collect_fn_mutants(&i.sig.output, &i.block.brace_token.span);
            syn::visit::visit_impl_item_method(self_, i)
        });
    }

    /// Visit `impl Foo { ...}` or `impl Debug for Foo { ... }`.
    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if attrs_excluded(&i.attrs) {
            return;
        }
        let type_name = type_name_string(&i.self_ty);
        let name = if let Some((_, trait_path, _)) = &i.trait_ {
            let trait_name = &trait_path.segments.last().unwrap().ident;
            if trait_name == "Default" {
                // We don't know (yet) how to generate an interestingly-broken
                // Default::default.
                return;
            }
            format!(
                "<impl {} for {}>",
                trait_name,
                remove_excess_spaces(&type_name)
            )
        } else {
            type_name
        };
        // Make an approximately-right namespace.
        // TODO: For `impl X for Y` get both X and Y onto the namespace
        // stack so that we can show a more descriptive name.
        self.in_namespace(&name, |v| syn::visit::visit_item_impl(v, i));
    }

    /// Visit `mod foo { ... }`.
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !attrs_excluded(&node.attrs) {
            self.in_namespace(&node.ident.to_string(), |v| {
                syn::visit::visit_item_mod(v, node)
            });
        }
    }
}

fn ops_for_return_type(return_type: &syn::ReturnType) -> Vec<MutationOp> {
    let mut ops: Vec<MutationOp> = Vec::new();
    match return_type {
        syn::ReturnType::Default => ops.push(MutationOp::Unit),
        syn::ReturnType::Type(_rarrow, box_typ) => match &**box_typ {
            syn::Type::Path(syn::TypePath { path, .. }) => {
                // dbg!(&path);
                if path.is_ident("bool") {
                    ops.push(MutationOp::True);
                    ops.push(MutationOp::False);
                } else if path.is_ident("String") {
                    // TODO: Detect &str etc.
                    ops.push(MutationOp::EmptyString);
                    ops.push(MutationOp::Xyzzy);
                } else if path_is_result(path) {
                    // TODO: Try this for any path ending in "Result".
                    // TODO: Recursively generate for types inside the Ok side of the Result.
                    ops.push(MutationOp::OkDefault);
                } else {
                    ops.push(MutationOp::Default)
                }
            }
            _ => ops.push(MutationOp::Default),
        },
    }
    ops
}

fn type_name_string(ty: &syn::Type) -> String {
    ty.to_token_stream().to_string()
}

fn return_type_to_string(return_type: &syn::ReturnType) -> String {
    match return_type {
        syn::ReturnType::Default => String::new(),
        syn::ReturnType::Type(arrow, typ) => {
            format!(
                "{} {}",
                arrow.to_token_stream(),
                remove_excess_spaces(&typ.to_token_stream().to_string())
            )
        }
    }
}

/// Convert a TokenStream representing a type to a String with typical Rust
/// spacing between tokens.
///
/// This shrinks for example "& 'static" to just "&'static".
fn remove_excess_spaces(type_str: &str) -> String {
    let mut c: Vec<char> = type_str.chars().collect();
    // Walk through looking at space characters, and consider whether we can drop them
    // without it being ambiguous.
    //
    // This is a bit hacky but seems to give reasonably legible results on
    // typical trees...
    let mut i = 1;
    while (i + 1) < c.len() {
        if c[i] == ' ' {
            let a = c[i - 1];
            let b = c[i + 1];
            if a == ':'
                || b == ':'
                || b == ','
                || a == '&'
                || a == '<'
                || b == '<'
                || a == '>'
                || b == '>'
            {
                c.remove(i);
                // Retry at the same i
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    c.into_iter().collect()
}

fn path_is_result(path: &syn::Path) -> bool {
    path.segments
        .last()
        .map(|segment| segment.ident == "Result")
        .unwrap_or_default()
}

/// True if any of the attrs indicate that we should skip this node and everything inside it.
fn attrs_excluded(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|attr| attr_is_cfg_test(attr) || attr_is_test(attr) || attr_is_mutants_skip(attr))
}

/// True if the block (e.g. the contents of a function) is empty.
fn block_is_empty(block: &syn::Block) -> bool {
    block.stmts.is_empty()
}

/// True if the attribute is `#[cfg(test)]`.
fn attr_is_cfg_test(attr: &Attribute) -> bool {
    if !attr.path.is_ident("cfg") {
        return false;
    }
    if let syn::Meta::List(meta_list) = attr.parse_meta().unwrap() {
        // We should have already checked this above, but to make sure:
        assert!(meta_list.path.is_ident("cfg"));
        for nested_meta in meta_list.nested {
            if let syn::NestedMeta::Meta(syn::Meta::Path(cfg_path)) = nested_meta {
                if cfg_path.is_ident("test") {
                    return true;
                }
            }
        }
    }
    false
}

/// True if the attribute is `#[test]`.
fn attr_is_test(attr: &Attribute) -> bool {
    attr.path.is_ident("test")
}

/// True if the attribute is `#[mutants::skip]`.
fn attr_is_mutants_skip(attr: &Attribute) -> bool {
    attr.path
        .segments
        .iter()
        .map(|ps| &ps.ident)
        .eq(["mutants", "skip"].iter())
}

#[cfg(test)]
mod test {
    #[test]
    fn path_is_result() {
        let path: syn::Path = syn::parse_quote! { Result<(), ()> };
        assert!(super::path_is_result(&path));
    }
}
