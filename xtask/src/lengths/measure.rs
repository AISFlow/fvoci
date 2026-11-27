//! Measures one Rust source: the lines of the file and of every function in it.

use anyhow::{Context as _, Result};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens as _;
use syn::ext::IdentExt as _;
use syn::spanned::Spanned as _;
use syn::visit::{self, Visit};

use super::{Length, Measured, Subject, TEST_MODULE, is_test_file};
use crate::files;

/// The length of the file `file` and of the functions written in it.
///
/// A function is measured from the line of its `fn` to the line of its closing brace,
/// with everything in between: comments, empty lines, closures, and the functions
/// inside it, which are measured on their own as well.
///
/// Not measured as functions: a function written inside the call or the definition of
/// a macro, and a closure that is no part of a function (the value of a `static`).
/// Their lines count in the file.
///
/// Refused, whatever the lengths: `rustfmt::skip`, under which a line holds as much as
/// one likes; code that is in a file the measure does not open (`include!`, a module
/// kept by `#[path]` in a file that is no `.rs`); and, outside the test directory of a
/// crate, what would take the limit of tests without being tests: a module named
/// `tests` that is not `#[cfg(test)]`, another module kept in a file `tests.rs`, and a
/// function, a type or a trait named `tests`.
///
/// # Errors
///
/// Fails if the source is not Rust that parses, or if one of its lines ends with a
/// carriage return alone.
pub(crate) fn measure_source(file: &str, source: &str) -> Result<Measured> {
    let parsed = syn::parse_file(source).map_err(|error| {
        let line = error.span().start().line;
        anyhow::Error::new(error).context(format!("line {line}"))
    })?;
    // A line ends with a line feed; a lone carriage return would hide the ones after.
    if source.replace("\r\n", "\n").contains('\r') {
        return Err(anyhow::anyhow!("a line ends with a carriage return alone"))
            .context("counting the lines");
    }
    let mut meter = Meter {
        file,
        test_names_are_free: is_test_file(file) && !file.ends_with("/tests.rs"),
        around: Vec::new(),
        measured: Measured {
            lengths: vec![Length {
                subject: Subject::File(file.to_owned()),
                lines: source.lines().count(),
                at: 1,
            }],
            refused: Vec::new(),
        },
    };
    meter.visit_file(&parsed);
    Ok(meter.measured)
}

struct Meter<'a> {
    file: &'a str,
    /// Whether the file is in the test directory of a crate, where everything has the
    /// limits of tests and the name `tests` decides nothing.
    test_names_are_free: bool,
    /// The names around the code being read: inline modules, the type of an `impl`,
    /// a trait, functions.
    around: Vec<String>,
    measured: Measured,
}

impl Meter<'_> {
    fn measure(&mut self, signature: &syn::Signature, body: &syn::Block) {
        let first = signature.fn_token.span.start().line;
        let last = body.brace_token.span.close().start().line;
        let mut name = self.around.clone();
        name.push(signature.ident.unraw().to_string());
        self.measured.lengths.push(Length {
            subject: Subject::Function {
                file: self.file.to_owned(),
                name: name.join("::"),
            },
            lines: last.saturating_sub(first) + 1,
            at: first,
        });
    }

    /// Reads what is inside something named `name`.
    fn inside(&mut self, name: String, read: impl FnOnce(&mut Self)) {
        self.around.push(name);
        read(self);
        self.around.pop();
    }

    /// Refuses the name of the test module on something that is no test module.
    fn keep_for_tests(&mut self, what: &str, name: &proc_macro2::Ident) {
        if name == TEST_MODULE && !self.test_names_are_free {
            self.measured.refused.push(format!(
                "{}:{}: a {what} is named `{TEST_MODULE}`: the name is kept for the \
                 #[cfg(test)] module, whose functions get the limit of tests",
                self.file,
                name.span().start().line
            ));
        }
    }
}

impl<'ast> Visit<'ast> for Meter<'_> {
    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        let mut names = Vec::new();
        collect_names(node.to_token_stream(), &mut names);
        if names
            .windows(2)
            .any(|pair| pair[0] == "rustfmt" && pair[1] == "skip")
        {
            self.measured.refused.push(format!(
                "{}:{}: rustfmt::skip: lengths are counted in lines, so the lines are \
                 the formatter's",
                self.file,
                node.span().start().line
            ));
        }
        visit::visit_attribute(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if node.path.is_ident("include") {
            self.measured.refused.push(format!(
                "{}:{}: include! brings in code that is not measured. Make it a module",
                self.file,
                node.bang_token.span.start().line
            ));
        }
        visit::visit_macro(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let of_tests = node.attrs.iter().any(is_cfg_test);
        if !of_tests {
            self.keep_for_tests("module that is not #[cfg(test)]", &node.ident);
        }
        if let Some(kept_in) = node.attrs.iter().find_map(files::path_attribute) {
            let line = node.ident.span().start().line;
            let file_name = kept_in.rsplit(['/', '\\']).next().unwrap_or(&kept_in);
            if std::path::Path::new(file_name)
                .extension()
                .is_none_or(|extension| extension != "rs")
            {
                self.measured.refused.push(format!(
                    "{}:{line}: the module {} is kept in {kept_in}, which is no .rs file and \
                     is not measured",
                    self.file, node.ident
                ));
            } else if file_name == "tests.rs"
                && !(of_tests && node.ident == TEST_MODULE)
                && !self.test_names_are_free
            {
                self.measured.refused.push(format!(
                    "{}:{line}: the module {} is kept in a file named tests.rs, which has the \
                     limits of tests: the name is kept for the #[cfg(test)] module `{TEST_MODULE}`",
                    self.file, node.ident
                ));
            }
        }
        self.inside(node.ident.unraw().to_string(), |meter| {
            visit::visit_item_mod(meter, node);
        });
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let of = type_name(&node.self_ty);
        if of == TEST_MODULE && !self.test_names_are_free {
            self.measured.refused.push(format!(
                "{}:{}: a type is named `{TEST_MODULE}`: the name is kept for the \
                 #[cfg(test)] module, whose functions get the limit of tests",
                self.file,
                node.impl_token.span.start().line
            ));
        }
        // `<Type as Trait<Argument>>`, so that the `from` of one conversion is not
        // taken for the `from` of another.
        let name = match &node.trait_ {
            Some((path, _)) => format!("<{of} as {}>", without_spaces(path)),
            None => of,
        };
        self.inside(name, |meter| visit::visit_item_impl(meter, node));
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.keep_for_tests("trait", &node.ident);
        self.inside(node.ident.unraw().to_string(), |meter| {
            visit::visit_item_trait(meter, node);
        });
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        self.keep_for_tests("type", &node.ident);
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        self.keep_for_tests("type", &node.ident);
        visit::visit_item_enum(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.keep_for_tests("function", &node.sig.ident);
        self.measure(&node.sig, &node.block);
        self.inside(node.sig.ident.unraw().to_string(), |meter| {
            visit::visit_item_fn(meter, node);
        });
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.keep_for_tests("function", &node.sig.ident);
        self.measure(&node.sig, &node.block);
        self.inside(node.sig.ident.unraw().to_string(), |meter| {
            visit::visit_impl_item_fn(meter, node);
        });
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.keep_for_tests("function", &node.sig.ident);
        if let Some(body) = &node.default {
            self.measure(&node.sig, body);
        }
        self.inside(node.sig.ident.unraw().to_string(), |meter| {
            visit::visit_trait_item_fn(meter, node);
        });
    }
}

/// Whether an attribute is `#[cfg(test)]`, or a `cfg` that asks for `test` among other
/// things (`all(test, unix)`). A `test` inside `not(…)` does not count.
fn is_cfg_test(attribute: &syn::Attribute) -> bool {
    fn asks_for_test(tokens: TokenStream) -> bool {
        let mut negated = false;
        tokens.into_iter().any(|tree| match tree {
            TokenTree::Ident(word) => {
                negated = word == "not";
                word == "test"
            }
            TokenTree::Group(group) => {
                !std::mem::take(&mut negated) && asks_for_test(group.stream())
            }
            TokenTree::Punct(_) | TokenTree::Literal(_) => false,
        })
    }
    match &attribute.meta {
        syn::Meta::List(list) if list.path.is_ident("cfg") => asks_for_test(list.tokens.clone()),
        _ => false,
    }
}

/// Every name among `tokens`, at any depth, in the order they are written.
fn collect_names(tokens: TokenStream, names: &mut Vec<String>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(name) => names.push(name.to_string()),
            TokenTree::Group(group) => collect_names(group.stream(), names),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

/// A trait as it is written, without the spaces the tokens are printed with.
fn without_spaces(path: &syn::Path) -> String {
    path.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect()
}

/// The name of the type an `impl` is for, without its path and its arguments; `_` for a
/// type that has none (a tuple, a slice).
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => path.path.segments.last().map_or_else(
            || "_".to_owned(),
            |segment| segment.ident.unraw().to_string(),
        ),
        syn::Type::Reference(reference) => type_name(&reference.elem),
        syn::Type::Paren(inner) => type_name(&inner.elem),
        syn::Type::Group(inner) => type_name(&inner.elem),
        _ => "_".to_owned(),
    }
}
