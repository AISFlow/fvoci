//! The source files the checks read, and how they are named in lists and messages.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Every `.rs` file under `directory`, in the order of their paths.
///
/// # Errors
///
/// Fails if a directory cannot be read, or holds a symbolic link: a check reads the
/// files where they are, and what a link leads to would be read twice, or never.
pub(crate) fn rust_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect(directory, &mut files)?;
    Ok(files)
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("reading {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .with_context(|| format!("reading {}", directory.display()))?;
    entries.sort();
    for path in entries {
        let kind = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading {}", path.display()))?
            .file_type();
        if kind.is_symlink() {
            bail!("{} is a symbolic link", path.display());
        } else if kind.is_dir() {
            collect(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// A path below the workspace root as the lists and the messages write it: from the
/// root, with `/` on every system.
pub(crate) fn shown_path(root: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// The value of `#[path = "…"]`: the file a module is kept in, where it is not the one
/// its name gives.
pub(crate) fn path_attribute(attribute: &syn::Attribute) -> Option<String> {
    if !attribute.path().is_ident("path") {
        return None;
    }
    match &attribute.meta {
        syn::Meta::NameValue(syn::MetaNameValue {
            value:
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }),
            ..
        }) => Some(text.value()),
        _ => None,
    }
}

/// What the tests of the checks share.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};

    /// An empty directory for one test's files, under the workspace's `target/`.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let directory = crate::workspace_root()
            .join("target")
            .join("tmp")
            .join("xtask")
            .join(name);
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    /// Writes the file `file` under `root`, with the directories it needs.
    pub(crate) fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().expect("the file is in a directory"))
            .expect("the directory is created");
        std::fs::write(path, text).expect("the file is written");
    }
}
