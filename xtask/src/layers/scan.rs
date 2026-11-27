//! Reads Rust source and lists what it names: the paths it writes, resolved to where
//! they lead, the names it gives to paths, and the handles it reaches through.
//!
//! The reading is of the text alone, without types. What it sees and what it cannot see
//! is said at [`read_source`].

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use syn::ext::IdentExt as _;
use syn::visit::{self, Visit};

/// Something a module names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reference {
    /// The line it is written on, from 1.
    pub(crate) line: usize,
    /// The module the code is in, inline modules included: `fvoci_server::api::tests`.
    pub(crate) module: String,
    /// What is named.
    pub(crate) target: Target,
}

/// What a [`Reference`] names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// An item by its path, as `use` or a call writes it.
    Path {
        /// The path, made absolute: `fvoci_store::Store`, `fvoci_server::ws::Hub`.
        /// Where its first name is one the file gave with `use` or `type`, it starts
        /// with that name (`fvoci_server::api::Database::connect`).
        path: String,
        /// Whether the first name is none the file knows, so that the path is kept as
        /// written: it starts at another crate, at a local, or at a name that an
        /// import of everything (`use super::*`) brought in.
        open: bool,
    },
    /// A handle taken from a value: `.store()` or the field `.store`, with the method
    /// called on it where there is one (`.store().get_block`).
    Handle {
        /// The name of the accessor or the field.
        name: String,
        /// How the code reads, without the receiver and the arguments.
        written: String,
    },
    /// `include!`: code of this module that is in a file the reading does not open.
    Included,
}

/// A name a module gives to a path, with `use` or `type`: `fvoci_server::app::Database`
/// for `fvoci_store::Store`. Whoever names the first names the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alias {
    pub(crate) name: String,
    pub(crate) target: String,
    /// Whether the name is given inside the body of a function, and so only there.
    pub(crate) in_body: bool,
}

/// A module that a file declares and keeps in another file (`mod name;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declared {
    /// The module's path: `fvoci_server::api::contract`.
    pub(crate) module: Vec<String>,
    /// The inline modules between the file and the declaration, outermost first.
    pub(crate) inline_parents: Vec<String>,
    /// The value of `#[path = "…"]`, if the declaration has one.
    pub(crate) path_attribute: Option<String>,
}

/// What one file holds.
#[derive(Debug, Default)]
pub(crate) struct Source {
    pub(crate) references: Vec<Reference>,
    pub(crate) aliases: Vec<Alias>,
    pub(crate) declared: Vec<Declared>,
    /// The modules the file holds: its own, and the inline ones (`mod name { … }`)
    /// that are not written inside the body of a function.
    pub(crate) modules: Vec<String>,
    /// The modules the file declares, inline or not, that are not there everywhere
    /// their name may be written: under a condition (`#[cfg(…)]`), or inside the body
    /// of a function.
    pub(crate) conditional: Vec<String>,
}

/// Reads the source of the module `module` (its path from the crate's name on).
///
/// Seen: every path in `use` (renamed, grouped, globbed), in expressions, types,
/// patterns, bounds and attributes, a path written as the text of an attribute's
/// argument (`#[serde(with = "a::b")]`), and what reads like a path among the tokens of
/// a macro. `crate`, `self`, `super`, the modules declared in the file and the names
/// the file gives with `use` and `type` are resolved, a name used alone included
/// (`Store`, after `use fvoci_store::Store`); an inline module also sees those of the
/// modules around it, as a test module does through `use super::*`. A handle is seen as
/// `.name()` without arguments or as the field `.name`, anywhere in the file, macros
/// included.
///
/// Not seen: a type that arrives without being named, as the result of a function or
/// inside another type; a handle kept in a variable and used from there, taken out of
/// a struct by a pattern, or reached under another name; code that a macro writes.
///
/// # Errors
///
/// Fails if the source is not Rust that parses.
pub(crate) fn read_source(module: &[String], source: &str, handles: &[&str]) -> Result<Source> {
    let file = syn::parse_file(source).map_err(|error| {
        let line = error.span().start().line;
        anyhow::Error::new(error).context(format!("line {line}"))
    })?;
    let mut reader = Reader::default();
    reader.modules.push(module.join("::"));
    reader.read_items(module.to_vec(), &file.items, &file.attrs);
    let tokens: TokenStream = source
        .parse()
        .map_err(|error| anyhow!("the tokens do not read: {error}"))?;
    let mut read = reader.finish();
    read_handles(tokens, handles, &module.join("::"), &mut read.references);
    read.references.sort_by_key(|reference| reference.line);
    Ok(read)
}

/// Whether an attribute may take the item it is on away: `#[cfg(…)]`, and
/// `#[cfg_attr(…)]`, which may hold one.
fn is_conditional(attribute: &syn::Attribute) -> bool {
    attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
}

/// Where a written path starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// At what is in sight where it is written.
    Scope,
    /// At the crates: `::name::…`.
    Crates,
}

impl Start {
    /// Where a path starts that syn has parsed.
    fn of(leading_colon: Option<&syn::token::PathSep>) -> Self {
        leading_colon.map_or(Self::Scope, |_| Self::Crates)
    }
}

/// Whether a text among tokens may be a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Text {
    /// In the arguments of an attribute: `with = "a::b"`.
    MayBePath,
    /// In the input of a macro: a message, a format.
    IsText,
}

/// The words of the language that no path among tokens starts with: after one of them
/// a path begins (`as ::a::B`, `impl ::a::B`).
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "do", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "macro", "match", "mod",
    "move", "mut", "pub", "ref", "return", "static", "struct", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "yield",
];

/// A module whose items are being read, with the names of the modules declared in it.
struct Scope {
    module: Vec<String>,
    children: BTreeSet<String>,
    /// Which of [`Reader::sights`] is this module's.
    sight: usize,
}

/// A path as the file writes it, resolved as far as the modules tell.
struct Written {
    line: usize,
    /// Which of [`Reader::sights`] the path is written in.
    sight: usize,
    /// The path: absolute, or as written where its first name is not known yet.
    names: Vec<String>,
    /// Whether the first name may be one the file gave with `use` or `type`.
    open: bool,
    /// The name this path is given, if it is the target of a `use` or part of a `type`.
    alias: Option<String>,
    /// Whether it is written inside the body of a function.
    in_body: bool,
}

#[derive(Default)]
struct Reader {
    /// The module of the file, then the inline modules down to the code being read.
    scopes: Vec<Scope>,
    /// For every module of the file: the module, then the modules around it in the
    /// file, outwards. The names given in any of them are in sight in the first.
    sights: Vec<Vec<String>>,
    written: Vec<Written>,
    /// The lines of `include!`.
    included: Vec<usize>,
    declared: Vec<Declared>,
    /// What becomes [`Source::modules`] and [`Source::conditional`].
    modules: Vec<String>,
    conditional: Vec<String>,
    /// The name the paths being read are given: inside `type Name = …`.
    naming: Option<String>,
    /// How many bodies of functions, and other blocks, are around the code being read.
    bodies: usize,
}

impl Reader {
    fn read_items(
        &mut self,
        module: Vec<String>,
        items: &[syn::Item],
        attributes: &[syn::Attribute],
    ) {
        let children = items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) => Some(module.ident.unraw().to_string()),
                _ => None,
            })
            .collect();
        let mut sight = vec![module.join("::")];
        sight.extend(
            self.scopes
                .iter()
                .rev()
                .map(|scope| scope.module.join("::")),
        );
        self.sights.push(sight);
        self.scopes.push(Scope {
            module,
            children,
            sight: self.sights.len() - 1,
        });
        for attribute in attributes {
            self.visit_attribute(attribute);
        }
        for item in items {
            self.visit_item(item);
        }
        self.scopes.pop();
    }

    /// The module of the code being read.
    fn module(&self) -> &[String] {
        self.scopes.last().map_or(&[], |scope| &scope.module)
    }

    /// Makes a path absolute as far as the modules tell: the path, and whether its
    /// first name is still open. `None` for a path that leads nowhere outside what it
    /// is written in (`Self::…`, more `super` than there are modules).
    fn resolve(&self, start: Start, names: &[String]) -> Option<(Vec<String>, bool)> {
        let (first, mut rest) = names.split_first()?;
        if start == Start::Crates {
            return Some((names.to_vec(), false));
        }
        let mut path = match first.as_str() {
            "Self" => return None,
            "crate" => vec![self.scopes.first()?.module.first()?.clone()],
            "self" => self.module().to_vec(),
            "super" => {
                let mut path = self.module().to_vec();
                path.pop()?;
                path
            }
            // A module declared here, or around an inline module.
            _ => match self
                .scopes
                .iter()
                .rev()
                .find(|scope| scope.children.contains(first))
            {
                Some(scope) => {
                    let mut path = scope.module.clone();
                    path.push(first.clone());
                    path
                }
                None => return Some((names.to_vec(), true)),
            },
        };
        if matches!(first.as_str(), "self" | "super") {
            // `super::super::a`, and `self::super::a`.
            while let Some((next, after)) = rest.split_first()
                && next == "super"
            {
                path.pop()?;
                rest = after;
            }
            if path.is_empty() {
                return None;
            }
        }
        path.extend(rest.iter().cloned());
        Some((path, false))
    }

    /// Notes a path. `alias` is the name a `use` gives it.
    fn record(&mut self, start: Start, names: &[String], line: usize, alias: Option<&str>) {
        let Some((names, open)) = self.resolve(start, names) else {
            return;
        };
        let alias = alias
            .map(|name| format!("{}::{name}", self.module().join("::")))
            .or_else(|| self.naming.clone());
        self.written.push(Written {
            line,
            sight: self.scopes.last().map_or(0, |scope| scope.sight),
            names,
            open,
            alias,
            in_body: self.bodies > 0,
        });
    }

    /// Notes the path of an import. One name alone is a crate there, where it is no
    /// module in sight: `use fvoci_store as database;`.
    fn record_import(&mut self, start: Start, names: &[String], line: usize, alias: Option<&str>) {
        let start = match self.resolve(start, names) {
            Some((_, true)) if names.len() == 1 => Start::Crates,
            _ => start,
        };
        self.record(start, names, line, alias);
    }

    fn read_use(&mut self, start: Start, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.unraw().to_string());
                self.read_use(start, prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Name(syn::UseName { ident }) => {
                self.record_used(start, prefix, ident, Some(ident));
            }
            syn::UseTree::Rename(syn::UseRename { ident, rename, .. }) => {
                self.record_used(start, prefix, ident, (rename != "_").then_some(rename));
            }
            syn::UseTree::Glob(glob) => {
                prefix.push("*".to_owned());
                self.record(start, prefix, glob.star_token.span.start().line, None);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.read_use(start, prefix, tree);
                }
            }
        }
    }

    /// Notes the last name of a `use`: `prefix::name`, given the name `alias`.
    fn record_used(
        &mut self,
        start: Start,
        prefix: &mut Vec<String>,
        name: &proc_macro2::Ident,
        alias: Option<&proc_macro2::Ident>,
    ) {
        let line = name.span().start().line;
        // `use a::{self}` names `a`, and gives it the name `a`.
        if name == "self" && !prefix.is_empty() {
            let alias = match alias {
                Some(alias) if alias != "self" => Some(alias.unraw().to_string()),
                Some(_) => prefix.last().cloned(),
                None => None,
            };
            self.record_import(start, prefix, line, alias.as_deref());
        } else {
            let alias = alias.map(|alias| alias.unraw().to_string());
            prefix.push(name.unraw().to_string());
            self.record_import(start, prefix, line, alias.as_deref());
            prefix.pop();
        }
    }

    /// Finds what reads like a path among tokens that are not parsed: the input of a
    /// macro, the arguments of an attribute.
    fn read_tokens(&mut self, tokens: TokenStream, text: Text) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let mut at = 0;
        while let Some(tree) = trees.get(at) {
            let here = at;
            at += 1;
            match tree {
                TokenTree::Group(group) => self.read_tokens(group.stream(), text),
                TokenTree::Literal(literal) if text == Text::MayBePath => {
                    self.read_text(&literal.to_string(), literal.span().start().line);
                }
                TokenTree::Ident(first) if !KEYWORDS.contains(&first.to_string().as_str()) => {
                    let mut names = vec![first.unraw().to_string()];
                    let mut end = here;
                    while is_path_separator(&trees, end + 1)
                        && let Some(TokenTree::Ident(next)) = trees.get(end + 3)
                    {
                        names.push(next.unraw().to_string());
                        end += 3;
                    }
                    at = end + 1;
                    if is_member(&trees, here) {
                        continue;
                    }
                    // After `::` with no name before it: `::a::b`, and what goes on
                    // from `<T as Trait>::`, which leads to no layer.
                    let start = if here >= 2 && is_path_separator(&trees, here - 2) {
                        Start::Crates
                    } else {
                        Start::Scope
                    };
                    // `Vec::<T>::new`: one name after `>::` is no path of its own.
                    if start == Start::Crates && names.len() < 2 {
                        continue;
                    }
                    let line = first.span().start().line;
                    let after = trees
                        .get(end + 3)
                        .filter(|_| is_path_separator(&trees, end + 1));
                    match after {
                        // `use a::{b, c::d};` inside a macro.
                        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
                            self.read_used_tokens(start, &mut names, group.stream(), line);
                            at = end + 4;
                        }
                        Some(TokenTree::Punct(star)) if star.as_char() == '*' => {
                            names.push("*".to_owned());
                            self.record(start, &names, line, None);
                        }
                        // `use a;` and `use a as b;` inside a macro.
                        _ if is_ident(&trees, here.wrapping_sub(1), "use") => {
                            self.record_import(start, &names, line, None);
                        }
                        _ => self.record(start, &names, line, None),
                    }
                }
                TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }

    /// Reads the braces of a `use` among tokens: `prefix::{…}`.
    fn read_used_tokens(
        &mut self,
        start: Start,
        prefix: &mut Vec<String>,
        tokens: TokenStream,
        line: usize,
    ) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let is_comma =
            |tree: &TokenTree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ',');
        for item in trees.split(is_comma) {
            let depth = prefix.len();
            let mut alias = None;
            let mut nested = false;
            let mut tokens = item.iter();
            while let Some(tree) = tokens.next() {
                match tree {
                    TokenTree::Ident(word) if word == "as" => {
                        alias = tokens.next().map(ToString::to_string);
                    }
                    TokenTree::Ident(word) if word == "self" && prefix.len() == depth => {}
                    TokenTree::Ident(name) => prefix.push(name.unraw().to_string()),
                    TokenTree::Punct(star) if star.as_char() == '*' => prefix.push("*".to_owned()),
                    TokenTree::Group(group) => {
                        self.read_used_tokens(start, prefix, group.stream(), line);
                        nested = true;
                    }
                    TokenTree::Punct(_) | TokenTree::Literal(_) => {}
                }
            }
            if !nested && !item.is_empty() {
                let alias = alias
                    .or_else(|| prefix.last().cloned())
                    .filter(|name| name != "*" && name != "_");
                self.record(start, prefix, line, alias.as_deref());
            }
            prefix.truncate(depth);
        }
    }

    /// Notes the text of an attribute's argument if it is a path of two names or more.
    fn read_text(&mut self, literal: &str, line: usize) {
        let Some(text) = literal
            .strip_prefix('"')
            .and_then(|text| text.strip_suffix('"'))
        else {
            return;
        };
        let (start, text) = match text.strip_prefix("::") {
            Some(rest) => (Start::Crates, rest),
            None => (Start::Scope, text),
        };
        let names: Vec<String> = text.split("::").map(str::to_owned).collect();
        let is_name = |name: &String| {
            !name.is_empty()
                && !name.starts_with(|first: char| first.is_ascii_digit())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        };
        if names.len() >= 2 && names.iter().all(is_name) {
            self.record(start, &names, line, None);
        }
    }

    /// Resolves the first names that were left open against the names the file gives,
    /// now that all of them are known.
    fn finish(self) -> Source {
        let given: BTreeSet<&str> = self
            .written
            .iter()
            .filter_map(|written| written.alias.as_deref())
            .collect();
        let mut source = Source {
            declared: self.declared,
            modules: self.modules,
            conditional: self.conditional,
            ..Source::default()
        };
        for written in &self.written {
            let sight = self
                .sights
                .get(written.sight)
                .map_or(&[][..], Vec::as_slice);
            let starts_at = written
                .names
                .first()
                .filter(|_| written.open)
                .and_then(|first| {
                    sight
                        .iter()
                        .map(|module| format!("{module}::{first}"))
                        .find(|name| given.contains(name.as_str()))
                })
                // A name is not given to itself: `use a::a;`.
                .filter(|name| written.alias.as_ref() != Some(name));
            let path = match &starts_at {
                Some(name) => {
                    let mut path = vec![name.clone()];
                    path.extend(written.names.iter().skip(1).cloned());
                    path.join("::")
                }
                None => written.names.join("::"),
            };
            if let Some(name) = &written.alias {
                source.aliases.push(Alias {
                    name: name.clone(),
                    target: path.clone(),
                    in_body: written.in_body,
                });
            }
            source.references.push(Reference {
                line: written.line,
                module: sight.first().cloned().unwrap_or_default(),
                target: Target::Path {
                    path,
                    open: written.open && starts_at.is_none(),
                },
            });
        }
        let module = self.sights.first().and_then(|sight| sight.first());
        for line in self.included {
            source.references.push(Reference {
                line,
                module: module.cloned().unwrap_or_default(),
                target: Target::Included,
            });
        }
        source
    }
}

impl<'ast> Visit<'ast> for Reader {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let mut module = self.module().to_vec();
        module.push(node.ident.unraw().to_string());
        if self.bodies > 0 || node.attrs.iter().any(is_conditional) {
            self.conditional.push(module.join("::"));
        }
        if let Some((_, items)) = &node.content {
            if self.bodies == 0 {
                self.modules.push(module.join("::"));
            }
            self.read_items(module, items, &node.attrs);
            return;
        }
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        let inline_parents = self
            .scopes
            .iter()
            .skip(1)
            .filter_map(|scope| scope.module.last().cloned())
            .collect();
        self.declared.push(Declared {
            module,
            inline_parents,
            path_attribute: node.attrs.iter().find_map(crate::files::path_attribute),
        });
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.bodies += 1;
        visit::visit_block(self, node);
        self.bodies -= 1;
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.read_use(
            Start::of(node.leading_colon.as_ref()),
            &mut Vec::new(),
            &node.tree,
        );
    }

    fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
        let name = node.ident.unraw().to_string();
        let alias = match &node.rename {
            Some((_, rename)) if rename != "_" => rename.unraw().to_string(),
            _ => name.clone(),
        };
        let line = node.ident.span().start().line;
        if name == "self" {
            // `extern crate self as name;` gives the crate itself a name.
            self.record(Start::Scope, &["crate".to_owned()], line, Some(&alias));
        } else {
            self.record(Start::Crates, &[name], line, Some(&alias));
        }
    }

    /// `type Name = …` gives `Name` to every path of the type it stands for.
    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        let name = format!("{}::{}", self.module().join("::"), node.ident.unraw());
        let outer = self.naming.replace(name);
        visit::visit_item_type(self, node);
        self.naming = outer;
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        if let Some(first) = node.segments.first() {
            let names: Vec<String> = node
                .segments
                .iter()
                .map(|segment| segment.ident.unraw().to_string())
                .collect();
            let start = Start::of(node.leading_colon.as_ref());
            self.record(start, &names, first.ident.span().start().line, None);
        }
        visit::visit_path(self, node);
    }

    /// `pub(crate)` and `pub(in path)` say who may see an item, not what the item uses.
    fn visit_visibility(&mut self, _: &'ast syn::Visibility) {}

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.visit_path(&node.path);
        if node.path.is_ident("include") {
            self.included.push(node.bang_token.span.start().line);
        }
        self.read_tokens(node.tokens.clone(), Text::IsText);
    }

    fn visit_meta_list(&mut self, node: &'ast syn::MetaList) {
        self.visit_path(&node.path);
        self.read_tokens(node.tokens.clone(), Text::MayBePath);
    }
}

fn is_ident(trees: &[TokenTree], at: usize, word: &str) -> bool {
    matches!(trees.get(at), Some(TokenTree::Ident(ident)) if ident == word)
}

fn is_punct(trees: &[TokenTree], at: usize, character: char) -> bool {
    matches!(trees.get(at), Some(TokenTree::Punct(punct)) if punct.as_char() == character)
}

/// Whether the two tokens from `at` are `::`.
fn is_path_separator(trees: &[TokenTree], at: usize) -> bool {
    matches!(trees.get(at), Some(TokenTree::Punct(punct))
        if punct.as_char() == ':' && punct.spacing() == Spacing::Joint)
        && is_punct(trees, at + 1, ':')
}

/// Whether the token at `at` comes after the dot of a member (`value.name`), which is
/// not the second dot of a range (`a..name`).
fn is_member(trees: &[TokenTree], at: usize) -> bool {
    at >= 1 && is_punct(trees, at - 1, '.') && !(at >= 2 && is_punct(trees, at - 2, '.'))
}

/// Finds `.name()` and `.name` for the names in `handles`, at any depth.
fn read_handles(tokens: TokenStream, handles: &[&str], module: &str, found: &mut Vec<Reference>) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (at, tree) in trees.iter().enumerate() {
        if let TokenTree::Group(group) = tree {
            read_handles(group.stream(), handles, module, found);
        }
        let TokenTree::Ident(ident) = tree else {
            continue;
        };
        let name = ident.unraw().to_string();
        if !is_member(&trees, at) || !handles.contains(&name.as_str()) {
            continue;
        }
        let (mut written, next) = match trees.get(at + 1) {
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis => {
                // A method of that name that takes arguments is something else
                // (`AtomicBool::store`).
                if !group.stream().is_empty() {
                    continue;
                }
                (format!(".{name}()"), at + 2)
            }
            _ => (format!(".{name}"), at + 1),
        };
        if is_punct(&trees, next, '.')
            && let Some(TokenTree::Ident(method)) = trees.get(next + 1)
        {
            written.push('.');
            written.push_str(&method.unraw().to_string());
        }
        found.push(Reference {
            line: ident.span().start().line,
            module: module.to_owned(),
            target: Target::Handle { name, written },
        });
    }
}
