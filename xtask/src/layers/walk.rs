//! Finds the files of a crate: from its roots along the `mod` declarations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::scan::Declared;
use crate::files;

/// A file of a crate and the module it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleFile {
    pub(crate) module: Vec<String>,
    pub(crate) file: PathBuf,
}

/// Finds the files of the crate `name` whose sources are in `source_directory`: from
/// `lib.rs` and `main.rs` along the `mod` declarations, `#[path]` included. A program
/// (`main.rs`) is read as the module `main` of its library, which is where its paths
/// lead.
///
/// `read` is called for every file with its module, and answers the modules the file
/// declares.
///
/// # Errors
///
/// Fails if a declared module has no file, if `read` fails, or if a `.rs` file in the
/// directory is no module of the crate: what the walk does not reach is not checked.
pub(crate) fn walk_crate(
    name: &str,
    source_directory: &Path,
    read: &mut dyn FnMut(&ModuleFile) -> Result<Vec<Declared>>,
) -> Result<()> {
    let mut reached = BTreeSet::new();
    let mut waiting = Vec::new();
    for (root, module) in [
        ("lib.rs", vec![name.to_owned()]),
        ("main.rs", vec![name.to_owned(), "main".to_owned()]),
    ] {
        let file = source_directory.join(root);
        if file.is_file() {
            // The modules a root declares are next to it.
            waiting.push((ModuleFile { module, file }, source_directory.to_owned()));
        }
    }
    if waiting.is_empty() {
        bail!(
            "{} has neither lib.rs nor main.rs",
            source_directory.display()
        );
    }
    while let Some((module_file, children_directory)) = waiting.pop() {
        if !reached.insert(module_file.file.clone()) {
            // It would be read as one of the two, and checked by the rules of that one.
            bail!(
                "{} is declared as a module twice; the second time as {}",
                module_file.file.display(),
                module_file.module.join("::")
            );
        }
        let file_directory = module_file.file.parent().unwrap_or(source_directory);
        for declared in read(&module_file)? {
            waiting.push(locate(&declared, file_directory, &children_directory)?);
        }
    }
    let unreached: Vec<String> = files::rust_files(source_directory)?
        .iter()
        .filter(|file| !reached.contains(*file))
        .map(|file| file.display().to_string())
        .collect();
    if !unreached.is_empty() {
        bail!(
            "not a module of {name}, so not checked: {}",
            unreached.join(", ")
        );
    }
    Ok(())
}

/// The file of a declared module and the directory its own modules are in.
fn locate(
    declared: &Declared,
    file_directory: &Path,
    children_directory: &Path,
) -> Result<(ModuleFile, PathBuf)> {
    let name = declared
        .module
        .last()
        .context("a declared module has a name")?;
    let mut directory = children_directory.to_owned();
    directory.extend(&declared.inline_parents);
    let module = declared.module.clone();
    if let Some(path) = &declared.path_attribute {
        // The checks read the `.rs` files of a crate; a module kept under another
        // extension would be code that only the compiler reads.
        if Path::new(path)
            .extension()
            .is_none_or(|extension| extension != "rs")
        {
            bail!(
                "the module {} is kept in {path}, which is no .rs file",
                module.join("::")
            );
        }
        // Next to the declaring file; inside inline modules, in their directory.
        let base = if declared.inline_parents.is_empty() {
            file_directory
        } else {
            &directory
        };
        let file = base.join(path);
        if !file.is_file() {
            bail!(
                "the module {} has no file {}",
                module.join("::"),
                file.display()
            );
        }
        let children = file.parent().unwrap_or(base).to_owned();
        return Ok((ModuleFile { module, file }, children));
    }
    let flat = directory.join(format!("{name}.rs"));
    let nested = directory.join(name).join("mod.rs");
    let file = if flat.is_file() {
        flat
    } else if nested.is_file() {
        nested
    } else {
        bail!(
            "the module {} has neither {} nor {}",
            module.join("::"),
            flat.display(),
            nested.display()
        );
    };
    Ok((ModuleFile { module, file }, directory.join(name)))
}
