//! The two images and their names on GHCR: `ghcr.io/fvoci/fvoci-server` and
//! `ghcr.io/fvoci/fvoci-search`, tagged `sha-<commit>` (and later the version of a
//! release). Each machine builds and pushes its own processor's image under
//! `sha-<commit>-<arch>`; one list of both (a manifest list) then goes under
//! `sha-<commit>`. Nothing here pushes: [`print_push_plan`] prints the commands.

use std::path::Path;

use anyhow::{Result, bail};

use crate::steps::{self, capture, say};

/// Where the images go: the organization of the repository. The `source` label links a
/// package to the repository, whose access it then inherits; a new package is private.
const REGISTRY: &str = "ghcr.io/fvoci";

/// The names of one image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Name {
    /// The repository, `ghcr.io/fvoci/fvoci-server`.
    pub(crate) repository: String,
    /// The tag of this machine's build: `<repository>:sha-<commit>-<arch>`.
    pub(crate) local: String,
}

/// The names of both images of one commit, and what the build stamps into them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Names {
    pub(crate) server: Name,
    pub(crate) search: Name,
    /// The commit, in full, with `-dirty` for a tree with uncommitted changes: what the
    /// server reports as its `commit`.
    revision: String,
    /// The commit's time (`%cI`), what the server reports as its `buildDate`: a build of
    /// one commit says the same whenever it is made.
    created: String,
    /// `sha-<short commit>`, with `-dirty` for a tree with uncommitted changes.
    tag: String,
    /// The processor, as Docker names it: `arm64`, `amd64`.
    arch: String,
}

impl Names {
    /// The names for the commit the checkout is at, built on this machine.
    ///
    /// # Errors
    ///
    /// Fails if `git` or `docker` cannot say what they are asked.
    pub(crate) fn of_checkout(root: &Path) -> Result<Self> {
        let root = root.to_string_lossy();
        let revision = asked("git", &["-C", &root, "rev-parse", "HEAD"])?;
        let tree = if asked("git", &["-C", &root, "status", "--porcelain"])?.is_empty() {
            Tree::Committed
        } else {
            Tree::Changed
        };
        let created = asked("git", &["-C", &root, "log", "-1", "--format=%cI"])?;
        let arch = asked("docker", &["version", "--format", "{{.Server.Arch}}"])?;
        Ok(Self::new(&revision, &created, tree, &arch))
    }

    /// Names of a commit that is no checkout's, for the tests.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::new(
            "0123456789abcdef",
            "2026-11-19T03:00:00+09:00",
            Tree::Committed,
            "arm64",
        )
    }

    fn new(revision: &str, created: &str, tree: Tree, arch: &str) -> Self {
        let short = revision.get(..12).unwrap_or(revision);
        let tag = match tree {
            Tree::Committed => format!("sha-{short}"),
            Tree::Changed => format!("sha-{short}-dirty"),
        };
        let name = |image: &str| {
            let repository = format!("{REGISTRY}/{image}");
            Name {
                local: format!("{repository}:{tag}-{arch}"),
                repository,
            }
        };
        Self {
            server: name("fvoci-server"),
            search: name("fvoci-search"),
            revision: match tree {
                Tree::Committed => revision.to_owned(),
                Tree::Changed => format!("{revision}-dirty"),
            },
            created: created.to_owned(),
            tag,
            arch: arch.to_owned(),
        }
    }

    /// What the server built for these names reports as its `commit` in ping.
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    /// What the server built for these names reports as its `buildDate` in ping.
    pub(crate) fn created(&self) -> &str {
        &self.created
    }
}

/// Whether the checkout is the commit, or has changes the commit does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tree {
    Committed,
    Changed,
}

/// What a command printed, failing if it failed.
fn asked(program: &str, arguments: &[&str]) -> Result<String> {
    let answer = capture(program, arguments)?;
    if !answer.succeeded {
        bail!(
            "`{program} {}` failed: {}",
            arguments.join(" "),
            answer.error
        );
    }
    Ok(answer.output)
}

/// The build arguments of both images: the commit and its time, which the server reports
/// in ping and the labels carry, and the tag.
fn build_arguments(names: &Names) -> Vec<String> {
    let given = [
        format!("FVOCI_BUILD_HASH={}", names.revision),
        format!("FVOCI_BUILD_DATE={}", names.created),
        format!("FVOCI_CREATED={}", names.created),
        format!("FVOCI_VERSION={}", names.tag),
    ];
    given
        .into_iter()
        .flat_map(|argument| ["--build-arg".to_owned(), argument])
        .collect()
}

/// Builds both images for this machine's processor, with the commit and the time in
/// their labels.
///
/// # Errors
///
/// Fails if a build fails.
pub(crate) fn build(root: &Path, names: &Names) -> Result<()> {
    let common = build_arguments(names);
    let builds: [(&str, &Name, &[&str]); 2] = [
        (
            "the server's image",
            &names.server,
            &["--file", "docker/fvoci/Dockerfile", "."],
        ),
        (
            "the search server's image",
            &names.search,
            &["docker/search"],
        ),
    ];
    for (what, name, rest) in builds {
        say(&format!("\n==> building {what}, {}", name.local));
        let mut arguments = vec!["build", "--tag", &name.local];
        arguments.extend(common.iter().map(String::as_str));
        arguments.extend_from_slice(rest);
        steps::run(root, "docker", &arguments, &[("DOCKER_BUILDKIT", "1")])?;
    }
    Ok(())
}

/// Prints how the images of the checkout's commit go to GHCR. Runs nothing.
///
/// # Errors
///
/// Fails if `git` or `docker` cannot say what they are asked.
pub(crate) fn print_push_plan(root: &Path) -> Result<()> {
    let names = Names::of_checkout(root)?;
    say(&push_plan(&names));
    Ok(())
}

fn push_plan(names: &Names) -> String {
    let both = [&names.server, &names.search];
    let mut lines = Vec::new();
    if names.tag.ends_with("-dirty") {
        lines.push(
            "# WARNING: the checkout has changes no commit holds; commit them before pushing."
                .to_owned(),
        );
    }
    lines.extend([
        "# Sign in once per machine with a classic personal access token that has".to_owned(),
        "# write:packages (GHCR takes no fine-grained token):".to_owned(),
        "#   echo \"$GHCR_TOKEN\" | docker login ghcr.io --username <github user> --password-stdin"
            .to_owned(),
        format!(
            "# On this machine ({}), after `cargo xtask docker` has built and tried them:",
            names.arch
        ),
    ]);
    lines.extend(
        both.iter()
            .map(|name| format!("docker push {}", name.local)),
    );
    lines.push(
        "# On the machine of the other processor, the same with its tags. Then, once,".to_owned(),
    );
    lines.push("# the list of both under the commit's tag (and a release's version):".to_owned());
    for name in both {
        let repository = &name.repository;
        let tag = &names.tag;
        lines.push(format!(
            "docker buildx imagetools create --tag {repository}:{tag} \
             {repository}:{tag}-arm64 {repository}:{tag}-amd64"
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tags_name_the_commit_and_the_processor() {
        let names = Names::new("0123456789abcdef0123", "", Tree::Committed, "arm64");
        assert_eq!(
            names.server.local,
            "ghcr.io/fvoci/fvoci-server:sha-0123456789ab-arm64"
        );
        assert_eq!(names.search.repository, "ghcr.io/fvoci/fvoci-search");
        let dirty = Names::new("0123456789abcdef0123", "", Tree::Changed, "amd64");
        assert_eq!(
            dirty.search.local,
            "ghcr.io/fvoci/fvoci-search:sha-0123456789ab-dirty-amd64"
        );
        assert_eq!(names.revision(), "0123456789abcdef0123");
        assert_eq!(
            dirty.revision(),
            "0123456789abcdef0123-dirty",
            "a server built from changes no commit holds does not claim the commit"
        );
    }

    #[test]
    fn the_plan_pushes_this_machines_images_and_lists_both_processors() {
        let plan = push_plan(&Names::new(
            "0123456789abcdef",
            "",
            Tree::Committed,
            "arm64",
        ));
        assert!(plan.contains("docker push ghcr.io/fvoci/fvoci-server:sha-0123456789ab-arm64"));
        assert!(plan.contains(
            "imagetools create --tag ghcr.io/fvoci/fvoci-search:sha-0123456789ab \
             ghcr.io/fvoci/fvoci-search:sha-0123456789ab-arm64 \
             ghcr.io/fvoci/fvoci-search:sha-0123456789ab-amd64"
        ));
        assert!(!plan.lines().any(|line| line.starts_with("docker login")));
    }
}
