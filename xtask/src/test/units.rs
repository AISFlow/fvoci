//! The units of a test run, and the parts they are shared out into.
//!
//! A unit is one `cargo test` of one database: the unit tests of the libraries and
//! binaries (`libs`), the documentation tests (`doc`), or one integration test target,
//! by its name (`api`; a name two packages share is one unit that runs both). Every unit
//! keeps the run's choice of packages, so that no unit builds its dependencies with
//! other features than the build before it.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

/// The unit of the unit tests of the libraries and binaries.
pub(crate) const LIBS: &str = "libs";

/// The unit of the documentation tests.
pub(crate) const DOC: &str = "doc";

/// The services a unit's tests run against: the database of the store, and S3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Services {
    Sqlite,
    Postgres,
    SqliteS3,
    PostgresS3,
}

/// One `cargo test` of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unit {
    /// Its name on the command line (`--unit`) and in the time file.
    pub(crate) name: String,
    /// The arguments of `cargo test` that choose its targets: `--test api`.
    pub(crate) targets: Vec<String>,
    /// Arguments of the test harness it always runs with: `--ignored`.
    pub(crate) flags: Vec<String>,
    /// The harness's filter, which the names of a rerun take the place of.
    pub(crate) filter: Vec<String>,
    /// Which slice of its target's listed tests this subunit runs, when `--part` splits
    /// a large integration test target across shards.
    pub(crate) slice: Option<Part>,
    /// How many tests (`passed + failed + ignored`) `libtest` must account for when this
    /// subunit runs whole.
    pub(crate) expected_tests: Option<usize>,
    pub(crate) services: Services,
}

impl Unit {
    fn new(name: &str, targets: &[&str], services: Services) -> Self {
        Self {
            name: name.to_owned(),
            targets: targets.iter().map(|&target| target.to_owned()).collect(),
            flags: Vec::new(),
            filter: Vec::new(),
            slice: None,
            expected_tests: None,
            services,
        }
    }

    /// A unit of the tests a run leaves out unless it is asked for them (`#[ignore]`),
    /// with the harness's filter of them.
    pub(crate) fn of_ignored(
        name: &str,
        targets: &[&str],
        filter: &[&str],
        services: Services,
    ) -> Self {
        Self {
            flags: vec!["--ignored".to_owned()],
            filter: filter.iter().map(|&word| word.to_owned()).collect(),
            ..Self::new(name, targets, services)
        }
    }

    /// Whether it runs the documentation tests, which `--no-run` does not build.
    pub(crate) fn is_doc(&self) -> bool {
        self.targets.iter().any(|target| target == "--doc")
    }

    /// The arguments of the harness: its own, then the names `instead` if there are,
    /// else its filter.
    pub(crate) fn harness_arguments(&self, instead: &[String]) -> Vec<String> {
        let mut arguments = self.flags.clone();
        if instead.is_empty() {
            arguments.extend(self.filter.iter().cloned());
        } else {
            arguments.extend(instead.iter().cloned());
        }
        arguments
    }
}

/// What `cargo metadata` says of the workspace, as far as the units need it.
#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    test: bool,
    doctest: bool,
}

/// The kinds of target that are libraries.
const LIBRARY_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

/// What `cargo metadata` says of the workspace's own packages.
///
/// # Errors
///
/// Fails if cargo cannot be run or fails.
pub(crate) fn workspace_metadata(root: &Path, cargo: &std::ffi::OsStr) -> Result<String> {
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(root)
        .output()
        .context("starting `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` ended with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("reading what `cargo metadata` wrote")
}

/// The units of the packages `packages` (every package of the workspace if `None`), in
/// `metadata`, the output of `cargo metadata`: what `cargo test` of those packages runs,
/// shared out by target, each run against `services`.
///
/// # Errors
///
/// Fails if `metadata` is not what `cargo metadata` writes, or a test target is called
/// as a unit of its own is.
pub(crate) fn of_packages(
    metadata: &str,
    packages: Option<&[&str]>,
    services: Services,
) -> Result<Vec<Unit>> {
    let metadata: Metadata =
        serde_json::from_str(metadata).context("reading what `cargo metadata` wrote")?;
    let chosen = metadata
        .packages
        .iter()
        .filter(|package| packages.is_none_or(|names| names.contains(&package.name.as_str())));
    let mut libs = BTreeSet::new();
    let mut tests = BTreeSet::new();
    let mut doc = false;
    for target in chosen.flat_map(|package| &package.targets) {
        let library = target
            .kind
            .iter()
            .any(|kind| LIBRARY_KINDS.contains(&kind.as_str()));
        doc |= library && target.doctest;
        if !target.test {
            continue;
        }
        for kind in &target.kind {
            match kind.as_str() {
                "test" => {
                    tests.insert(target.name.clone());
                }
                "bin" => {
                    libs.insert("--bins".to_owned());
                }
                "example" => {
                    libs.insert(format!("--example={}", target.name));
                }
                "bench" => {
                    libs.insert(format!("--bench={}", target.name));
                }
                _ if library => {
                    libs.insert("--lib".to_owned());
                }
                _ => {}
            }
        }
    }
    let mut units = Vec::new();
    if !libs.is_empty() {
        let targets: Vec<&str> = libs.iter().map(String::as_str).collect();
        units.push(Unit::new(LIBS, &targets, services));
    }
    if doc {
        units.push(Unit::new(DOC, &["--doc"], services));
    }
    for test in tests {
        if test == LIBS || test == DOC {
            bail!("a test target is called `{test}`, as a unit of its own is: rename it");
        }
        units.push(Unit::new(&test, &["--test", &test], services));
    }
    Ok(units)
}

/// One of the parts the units of a run are shared out into: `2/4` is the second of four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Part {
    index: NonZeroUsize,
    count: NonZeroUsize,
}

impl Part {
    /// `index/count`, when `1 <= index <= count`.
    pub(crate) fn new(index: usize, count: usize) -> Option<Self> {
        let index = NonZeroUsize::new(index)?;
        let count = NonZeroUsize::new(count)?;
        (index <= count).then_some(Self { index, count })
    }

    /// Which part this is, from `1`.
    pub(crate) fn index(self) -> usize {
        self.index.get()
    }

    /// How many parts there are in total.
    pub(crate) fn count(self) -> usize {
        self.count.get()
    }
}

impl FromStr for Part {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let wrong = || format!("`{text}` is no part: give one as `2/4`, the second of four");
        let (index, count) = text.split_once('/').ok_or_else(wrong)?;
        let index: NonZeroUsize = index.trim().parse().map_err(|_| wrong())?;
        let count: NonZeroUsize = count.trim().parse().map_err(|_| wrong())?;
        if index > count {
            return Err(wrong());
        }
        Ok(Self { index, count })
    }
}

impl fmt::Display for Part {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.index, self.count)
    }
}

/// The units of `part`, of all the parts `units` are shared out into, in their order.
///
/// Each unit weighs the seconds `seconds` gives it; a unit it does not know weighs the
/// median of those it knows (the lower middle one of an even number), and every unit
/// weighs the same if it knows none. The heaviest unit goes first, each into the part
/// that weighs least so far. The same units and seconds always make the same parts,
/// every unit in exactly one.
pub(crate) fn partition(units: Vec<Unit>, seconds: &HashMap<String, f64>, part: Part) -> Vec<Unit> {
    let known = |unit: &Unit| {
        seconds
            .get(&unit.name)
            .copied()
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
    };
    let mut weights: Vec<f64> = units.iter().filter_map(known).collect();
    weights.sort_by(f64::total_cmp);
    // The lower of the two middle weights when there is an even number of them.
    let fallback = weights
        .get(weights.len().saturating_sub(1) / 2)
        .copied()
        .unwrap_or(1.0);
    let weight = |unit: &Unit| known(unit).unwrap_or(fallback);

    let mut order: Vec<usize> = (0..units.len()).collect();
    order.sort_by(|&a, &b| {
        weight(&units[b])
            .total_cmp(&weight(&units[a]))
            .then_with(|| units[a].name.cmp(&units[b].name))
    });
    let mut loads = vec![0.0_f64; part.count.get()];
    let mut assigned = vec![0; units.len()];
    for index in order {
        let lightest = loads
            .iter()
            .enumerate()
            .min_by(|(a, load_a), (b, load_b)| load_a.total_cmp(load_b).then(a.cmp(b)))
            .map_or(0, |(lightest, _)| lightest);
        loads[lightest] += weight(&units[index]);
        assigned[index] = lightest;
    }
    let wanted = part.index.get() - 1;
    units
        .into_iter()
        .zip(assigned)
        .filter(|(_, assigned)| *assigned == wanted)
        .map(|(unit, _)| unit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &str = r#"{"packages": [
        {"name": "one", "targets": [
            {"name": "one", "kind": ["lib"], "test": true, "doctest": true},
            {"name": "one-cli", "kind": ["bin"], "test": true, "doctest": false},
            {"name": "api", "kind": ["test"], "test": true, "doctest": false},
            {"name": "shared", "kind": ["test"], "test": true, "doctest": false},
            {"name": "fake", "kind": ["example"], "test": false, "doctest": false},
            {"name": "build-script-build", "kind": ["custom-build"], "test": false, "doctest": false}
        ]},
        {"name": "two", "targets": [
            {"name": "two", "kind": ["proc-macro"], "test": true, "doctest": false},
            {"name": "shared", "kind": ["test"], "test": true, "doctest": false},
            {"name": "speed", "kind": ["bench"], "test": true, "doctest": false}
        ]}
    ]}"#;

    fn names(units: &[Unit]) -> Vec<&str> {
        units.iter().map(|unit| unit.name.as_str()).collect()
    }

    #[test]
    fn every_target_cargo_test_runs_is_in_one_unit() {
        let units = of_packages(METADATA, None, Services::Sqlite).expect("units");
        assert_eq!(names(&units), ["libs", "doc", "api", "shared"]);
        assert_eq!(
            units[0].targets,
            ["--bench=speed", "--bins", "--lib"],
            "libraries, binaries and the bench that is tested; not the example that is not"
        );
        assert_eq!(units[1].targets, ["--doc"]);
        assert_eq!(units[2].targets, ["--test", "api"]);
        assert_eq!(
            units[3].targets,
            ["--test", "shared"],
            "one unit for the name both packages give a target"
        );
    }

    #[test]
    fn the_units_of_some_packages_are_theirs_only() {
        let units = of_packages(METADATA, Some(&["two"]), Services::Postgres).expect("units");
        assert_eq!(
            names(&units),
            ["libs", "shared"],
            "no documentation tests in two"
        );
        assert_eq!(units[0].targets, ["--bench=speed", "--lib"]);
    }

    #[test]
    fn a_test_target_called_as_a_unit_is_refused() {
        let metadata = r#"{"packages": [{"name": "p", "targets": [
            {"name": "doc", "kind": ["test"], "test": true, "doctest": false}]}]}"#;
        let error = of_packages(metadata, None, Services::Sqlite).expect_err("a test called doc");
        assert!(error.to_string().contains("`doc`"), "{error}");
    }

    #[test]
    fn the_harness_gets_the_names_of_a_rerun_instead_of_the_filter() {
        let unit = Unit::of_ignored("api", &["--test", "api"], &["with_s3"], Services::SqliteS3);
        assert_eq!(unit.harness_arguments(&[]), ["--ignored", "with_s3"]);
        let names = ["--exact".to_owned(), "api::a".to_owned()];
        assert_eq!(
            unit.harness_arguments(&names),
            ["--ignored", "--exact", "api::a"]
        );
    }

    #[test]
    fn parts_are_read_as_one_of_so_many() {
        let part: Part = "2/4".parse().expect("a part");
        assert_eq!(part.to_string(), "2/4");
        for wrong in ["0/4", "5/4", "2", "a/4", "2/0", ""] {
            assert!(wrong.parse::<Part>().is_err(), "{wrong:?} is no part");
        }
    }

    fn units(names: &[&str]) -> Vec<Unit> {
        names
            .iter()
            .map(|&name| Unit::new(name, &[], Services::Sqlite))
            .collect()
    }

    fn part(text: &str) -> Part {
        text.parse().expect("a part")
    }

    #[test]
    fn every_unit_is_in_exactly_one_part_whatever_the_times_say() {
        let all = units(&["a", "b", "c", "d", "e", "f", "g"]);
        let timed = HashMap::from([
            ("a".to_owned(), 30.0),
            ("c".to_owned(), 5.0),
            ("z".to_owned(), 99.0),
        ]);
        for seconds in [HashMap::new(), timed] {
            for count in 1..=9 {
                let mut seen: Vec<String> = (1..=count)
                    .flat_map(|index| {
                        partition(all.clone(), &seconds, part(&format!("{index}/{count}")))
                    })
                    .map(|unit| unit.name)
                    .collect();
                seen.sort();
                assert_eq!(
                    seen,
                    ["a", "b", "c", "d", "e", "f", "g"],
                    "{count} parts, {seconds:?}"
                );
            }
        }
    }

    #[test]
    fn the_heaviest_units_go_to_parts_of_their_own() {
        let all = units(&["api", "doc", "libs", "regression", "web"]);
        let seconds = HashMap::from([
            ("api".to_owned(), 300.0),
            ("regression".to_owned(), 200.0),
            ("doc".to_owned(), 20.0),
            ("libs".to_owned(), 30.0),
        ]);
        assert_eq!(
            names(&partition(all.clone(), &seconds, part("1/3"))),
            ["api"]
        );
        assert_eq!(
            names(&partition(all.clone(), &seconds, part("2/3"))),
            ["regression"]
        );
        assert_eq!(
            names(&partition(all, &seconds, part("3/3"))),
            ["doc", "libs", "web"],
            "web, unknown, weighs the median of the known (30 s), and the order of the units is kept"
        );
    }

    #[test]
    fn the_same_units_and_times_make_the_same_parts() {
        let all = units(&["x", "y", "z", "w"]);
        let seconds = HashMap::from([("x".to_owned(), 1.0), ("y".to_owned(), 1.0)]);
        let first = partition(all.clone(), &seconds, part("1/2"));
        let mut shuffled = all;
        shuffled.reverse();
        let mut second = partition(shuffled, &seconds, part("1/2"));
        second.reverse();
        assert_eq!(
            names(&first),
            names(&second),
            "the order the units come in does not matter"
        );
    }
}
