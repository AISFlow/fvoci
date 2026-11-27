//! `cargo xtask test`: every test, on SQLite and on PostgreSQL, and the S3 storage test
//! with the uploads in parts that go to S3 and the files of two organizations there.
//!
//! The tests of a database are built once and run in units ([`units`]): one
//! `cargo test` for the unit tests of the libraries and binaries, one for the
//! documentation tests, one for each integration test target. A unit that fails stops
//! nothing. The summary at the end gives each unit's time, names the tests that failed
//! and prints the command that runs them again (`--unit`, and the names after `--`); the
//! time file keeps the same ([`crate::times`]). `--database` takes one database, and
//! `--part 2/4` the second of four parts of its units (with `api` split in two), weighed
//! by the times of a time file given with `--times` or built-in weights, for machines
//! that share a run.
//!
//! The containers are prepared first unless the environment already names the services:
//! `FVOCI_STORE_TEST_CONN_STRING` for PostgreSQL, `FVOCI_S3_TEST_ENDPOINT` together with
//! `FVOCI_S3_TEST_BUCKET` for the object store (for CI, where the services are given).
//! A PostgreSQL URL given so must name a role that is no superuser, has `NOBYPASSRLS`
//! and has `CREATE` on the database: the tests refuse any other. With it goes
//! `FVOCI_STORE_TEST_SUPERUSER_CONN_STRING`, the URL of a superuser of the same
//! database, for the tests of what the server makes of such a role.
//! `FVOCI_TEST_CPUS` sets the number of test threads ([`crate::cpus`]).

mod harness;
mod report;
mod shards;
mod units;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};

use self::report::UnitResult;
pub(crate) use self::shards::base_name as base_unit_name;
pub(crate) use self::units::Part;
use self::units::{Services, Unit};
use crate::containers;
use crate::steps::{self, Step};
use crate::times::{self, BuildTime, Scope, Verdict};

/// The variables the store's test support reads.
const DB_TYPE_ENV: &str = "FVOCI_STORE_TEST_DB_TYPE";
const CONN_STRING_ENV: &str = "FVOCI_STORE_TEST_CONN_STRING";
const SUPERUSER_CONN_STRING_ENV: &str = "FVOCI_STORE_TEST_SUPERUSER_CONN_STRING";

/// The variable that makes the regression tests record instead of compare.
const REGRESSION_UPDATE_ENV: &str = "FVOCI_REGRESSION_UPDATE";

/// The variables the S3 storage test reads.
const S3_ENDPOINT_ENV: &str = "FVOCI_S3_TEST_ENDPOINT";
const S3_BUCKET_ENV: &str = "FVOCI_S3_TEST_BUCKET";

/// The packages whose tests run on PostgreSQL too.
const POSTGRES_PACKAGES: [&str; 2] = ["fvoci-store", "fvoci-server"];

fn env(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// What one database's tests run against, in the order a full run takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Database {
    Sqlite,
    Postgres,
    /// The S3 storage tests; one of them needs PostgreSQL too.
    S3,
}

impl Database {
    const ALL: [Self; 3] = [Self::Sqlite, Self::Postgres, Self::S3];

    /// Its name as `--database` takes it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::S3 => "s3",
        }
    }

    fn step(self) -> &'static str {
        match self {
            Self::Sqlite => "tests on SQLite",
            Self::Postgres => "tests on PostgreSQL",
            Self::S3 => "S3 storage test",
        }
    }

    /// The packages `cargo test` chooses for it; every unit keeps them.
    fn packages(self) -> &'static [&'static str] {
        match self {
            Self::Sqlite => &["--workspace"],
            Self::Postgres => &["--package", "fvoci-store", "--package", "fvoci-server"],
            Self::S3 => &["--package", "fvoci-server"],
        }
    }

    /// Its units, of the workspace `metadata` describes.
    fn units(self, metadata: &str) -> Result<Vec<Unit>> {
        match self {
            Self::Sqlite => units::of_packages(metadata, None, Services::Sqlite),
            Self::Postgres => {
                units::of_packages(metadata, Some(&POSTGRES_PACKAGES), Services::Postgres)
            }
            Self::S3 => Ok(vec![
                Unit::of_ignored("storage", &["--test", "storage"], &[], Services::SqliteS3),
                // The uploads in parts that go to S3 as multipart uploads.
                Unit::of_ignored("api", &["--test", "api"], &["with_s3"], Services::SqliteS3),
                // The files of two organizations of one server in one bucket.
                Unit::of_ignored(
                    "organization_boundary_round2",
                    &["--test", "organization_boundary_round2"],
                    &[
                        "--exact",
                        "media::s3_files_tokens_uploads_and_housekeeping_stay_in_their_organizations",
                    ],
                    Services::PostgresS3,
                ),
            ]),
        }
    }
}

/// Which units of the chosen databases run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Choice {
    Every,
    /// One part of the units, weighed by the time file `times` if one is given.
    Part {
        part: Part,
        times: Option<PathBuf>,
    },
    /// The units of these names, with these arguments for the test harness instead of
    /// their filter if there are any: the names of a rerun.
    Units {
        names: Vec<String>,
        arguments: Vec<String>,
    },
}

/// Runs the tests of `databases` (all three if `None`), the units `choice` takes.
///
/// # Errors
///
/// Fails if a container cannot be prepared or a test run failed.
pub(crate) fn run(root: &Path, databases: Option<Database>, choice: &Choice) -> Result<()> {
    // With it set the regression tests rewrite their snapshots instead of comparing,
    // and would do so once per database. This task verifies; it never records.
    if env(REGRESSION_UPDATE_ENV).is_some() {
        bail!(
            "{REGRESSION_UPDATE_ENV} is set: the regression tests would rewrite their \
             snapshots instead of checking them. Unset it; to record snapshots run \
             `{REGRESSION_UPDATE_ENV}=1 cargo test -p fvoci-server --test regression`"
        );
    }
    if databases.is_none() && *choice != Choice::Every {
        bail!(
            "--part and --unit choose units of one database: add --database sqlite, postgres or s3"
        );
    }
    let cargo = steps::cargo();
    let weights = match choice {
        Choice::Part {
            times: Some(file), ..
        } => {
            // Every machine that shares the run must weigh the units alike: a file one of
            // them lacks, or has cut short, fails it rather than making it share them out
            // otherwise; the digest is for the machines' logs to compare.
            let given = times::read_given(file).context("--times")?;
            steps::say(&format!(
                "    weighing the parts by {} records of {} (sha256 {})",
                given.records.len(),
                file.display(),
                given.sha256
            ));
            given.records
        }
        _ => Vec::new(),
    };
    let clock = times::Clock::start(root);
    let run = Run {
        root,
        metadata: units::workspace_metadata(root, &cargo)?,
        cargo,
        choice,
        weights,
        test_threads: crate::cpus::test_threads()?
            .map(|(name, value)| (name.to_owned(), value))
            .into_iter()
            .collect(),
        backends: Backends::prepare()?,
        results: RefCell::new(Vec::new()),
        builds: RefCell::new(Vec::new()),
    };

    let shared = &run;
    let parts = Database::ALL
        .into_iter()
        .map(|database| match databases {
            Some(chosen) if chosen != database => {
                Step::skipped(database.step(), &format!("--database {}", chosen.name()))
            }
            _ => Step::new(database.step(), move || shared.database(database)),
        })
        .collect();
    let ran = steps::run_all("test", parts);
    let Run {
        backends,
        results,
        builds,
        ..
    } = run;
    // The containers the steps started stop now, however the run ended.
    drop(backends);

    let results = results.into_inner();
    let earlier = times::read(&times::path(root)).unwrap_or_else(|error| {
        steps::say(&format!(
            "times not read, so no test is called flaky: {error:#}"
        ));
        Vec::new()
    });
    let mut record = times::Record::new("test", &clock, &ran);
    let flaky = report::flaky(&results, &times::failures_of(&earlier, &record.source));
    steps::say(&report::lines(&results, &flaky, times::now_unix()));
    record.builds = builds.into_inner();
    record.units = results.iter().map(UnitResult::time).collect();
    record.flaky = flaky.into_iter().map(|(name, _)| name).collect();
    times::append(root, &record);
    ran.into_result()
}

/// A run of the tests: what its steps share.
struct Run<'a> {
    root: &'a Path,
    cargo: OsString,
    metadata: String,
    choice: &'a Choice,
    /// The time file's records that weigh the parts.
    weights: Vec<times::Record>,
    test_threads: Vec<(String, String)>,
    backends: RefCell<Backends>,
    results: RefCell<Vec<UnitResult>>,
    builds: RefCell<Vec<BuildTime>>,
}

impl Run<'_> {
    /// Runs the chosen units of `database`, all of them, after one build.
    fn database(&self, database: Database) -> Result<()> {
        let all = database.units(&self.metadata)?;
        let mut chosen = self.choose(database, all.clone())?;
        let names: Vec<&str> = chosen.iter().map(|unit| unit.name.as_str()).collect();
        steps::say(&format!("    units: {}", names.join(", ")));
        if let Some(arguments) = build_arguments(database, &all, &chosen) {
            let started = Instant::now();
            let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
            let built = steps::run(self.root, &self.cargo, &arguments, &[]);
            self.builds.borrow_mut().push(BuildTime {
                database: database.name().to_owned(),
                verdict: if built.is_ok() {
                    Verdict::Passed
                } else {
                    Verdict::Failed
                },
                seconds: started.elapsed().as_secs_f64(),
            });
            built?;
        }
        self.populate_subunit_filters(database, &mut chosen)?;
        let instead = match self.choice {
            Choice::Units { arguments, .. } => arguments.as_slice(),
            _ => &[],
        };
        let mut failed = Vec::new();
        for unit in &chosen {
            ensure_subunit_filter(unit)?;
            let mut environment = self.backends.borrow_mut().environment(unit.services)?;
            environment.extend(self.test_threads.iter().cloned());
            let mut arguments: Vec<String> = ["test", "--locked", "--no-fail-fast"]
                .iter()
                .chain(database.packages())
                .map(|&argument| argument.to_owned())
                .collect();
            arguments.extend(unit.targets.iter().cloned());
            arguments.push("--".to_owned());
            arguments.extend(unit.harness_arguments(instead));
            let ran = harness::run(self.root, &self.cargo, &arguments, &environment)?;
            let result = UnitResult {
                database: database.name(),
                unit: unit.name.clone(),
                scope: if instead.is_empty() {
                    Scope::Whole
                } else {
                    Scope::Chosen
                },
                expected_tests: unit.expected_tests,
                run: ran,
            };
            if result.failed() {
                failed.push(unit.name.as_str());
            }
            self.results.borrow_mut().push(result);
        }
        let results = self.results.borrow();
        let of_database: Vec<&UnitResult> = results
            .iter()
            .filter(|result| result.database == database.name())
            .collect();
        let chose_wrongly = chose_wrongly(database, instead, &of_database);
        match (failed.is_empty(), chose_wrongly) {
            (true, None) => Ok(()),
            (true, Some(wrongly)) => bail!("{wrongly}"),
            (false, wrongly) => bail!(
                "{} of {} units failed: {}{}",
                failed.len(),
                chosen.len(),
                failed.join(", "),
                wrongly
                    .map(|wrongly| format!("; and {wrongly}"))
                    .unwrap_or_default()
            ),
        }
    }

    /// The units of `all` the choice of the run takes.
    fn choose(&self, database: Database, all: Vec<Unit>) -> Result<Vec<Unit>> {
        match self.choice {
            Choice::Every => Ok(all),
            Choice::Part { part, .. } => {
                let expanded = shards::expand_units(all, *part);
                let recorded = times::unit_seconds(&self.weights, database.name());
                let seconds = shards::weigh_units(&expanded, &recorded);
                Ok(units::partition(expanded, &seconds, *part))
            }
            Choice::Units { names, .. } => {
                if let Some(unknown) = names
                    .iter()
                    .find(|name| !all.iter().any(|unit| unit.name == **name))
                {
                    let known: Vec<&str> = all.iter().map(|unit| unit.name.as_str()).collect();
                    bail!(
                        "{} has no unit `{unknown}`; its units are {}",
                        database.name(),
                        known.join(", ")
                    );
                }
                Ok(all
                    .into_iter()
                    .filter(|unit| names.contains(&unit.name))
                    .collect())
            }
        }
    }

    /// Queries `-- --list --format terse` for any split subunit in `chosen` (already
    /// compiled by the build step), logs the slice count and list digest, and fills its
    /// exact `libtest` filter and expected test count.
    fn populate_subunit_filters(&self, database: Database, chosen: &mut [Unit]) -> Result<()> {
        let mut listed: HashMap<Vec<String>, Vec<String>> = HashMap::new();
        for unit in chosen {
            let Some(slice) = unit.slice else {
                continue;
            };
            if !listed.contains_key(&unit.targets) {
                let fetched =
                    shards::list_tests(self.root, &self.cargo, database.packages(), &unit.targets)?;
                listed.insert(unit.targets.clone(), fetched);
            }
            if let Some(tests) = listed.get(&unit.targets) {
                let prepared = shards::prepare_part(tests, slice);
                steps::say(&format!(
                    "    {}: {} of {} tests (list sha256 {})",
                    unit.name,
                    prepared.expected_tests,
                    prepared.total_tests,
                    &prepared.list_sha256[..12]
                ));
                unit.filter = prepared.filter;
                unit.expected_tests = Some(prepared.expected_tests);
            }
        }
        Ok(())
    }
}

/// Refuses to run a split subunit whose `filter` was not populated first, so a bug never
/// silently runs the whole binary in every shard.
fn ensure_subunit_filter(unit: &Unit) -> Result<()> {
    if unit.slice.is_some() && unit.filter.is_empty() {
        bail!(
            "subunit `{}` has no harness filter populated before running",
            unit.name
        );
    }
    Ok(())
}

/// The `cargo test --no-run` that builds what the chosen units run, if anything: all of
/// the database's packages' tests when every unit runs, as `cargo test` builds them,
/// else the targets of the chosen units. Documentation tests are built as they run.
fn build_arguments(database: Database, all: &[Unit], chosen: &[Unit]) -> Option<Vec<String>> {
    let mut arguments: Vec<String> = ["test", "--locked", "--no-run"]
        .iter()
        .chain(database.packages())
        .map(|&argument| argument.to_owned())
        .collect();
    if database != Database::S3 && chosen == all {
        return Some(arguments);
    }
    let mut seen: Vec<&[String]> = Vec::new();
    let mut targets = Vec::new();
    for unit in chosen.iter().filter(|unit| !unit.is_doc()) {
        if !seen.contains(&unit.targets.as_slice()) {
            seen.push(&unit.targets);
            targets.extend(unit.targets.iter().cloned());
        }
    }
    if targets.is_empty() {
        return None;
    }
    arguments.extend(targets);
    Some(arguments)
}

/// The services of the tests, prepared when a unit first needs them. Dropped, it stops
/// the containers the run started.
struct Backends {
    uses: containers::Uses,
    postgres: Option<containers::PostgresUrls>,
    object_store: Option<(String, String)>,
}

impl Backends {
    fn prepare() -> Result<RefCell<Self>> {
        Ok(RefCell::new(Self {
            uses: containers::Uses::new()?,
            postgres: None,
            object_store: None,
        }))
    }

    /// The variables that point the tests at `services`.
    fn environment(&mut self, services: units::Services) -> Result<Vec<(String, String)>> {
        let mut environment = Vec::new();
        let mut set =
            |name: &str, value: &str| environment.push((name.to_owned(), value.to_owned()));
        match services {
            // Whatever the caller's environment says, these tests run on SQLite.
            units::Services::Sqlite | units::Services::SqliteS3 => set(DB_TYPE_ENV, "sqlite"),
            units::Services::Postgres | units::Services::PostgresS3 => {
                let urls = self.postgres()?;
                set(DB_TYPE_ENV, "postgres");
                set(CONN_STRING_ENV, &urls.tests);
                set(SUPERUSER_CONN_STRING_ENV, &urls.superuser);
            }
        }
        if matches!(
            services,
            units::Services::SqliteS3 | units::Services::PostgresS3
        ) {
            let (endpoint, bucket) = self.object_store()?;
            set(S3_ENDPOINT_ENV, &endpoint);
            set(S3_BUCKET_ENV, &bucket);
        }
        Ok(environment)
    }

    /// The PostgreSQL of the environment, or of the container prepared for the tests.
    fn postgres(&mut self) -> Result<containers::PostgresUrls> {
        if let Some(urls) = &self.postgres {
            return Ok(urls.clone());
        }
        let urls = if let Some(url) = env(CONN_STRING_ENV) {
            steps::say(&format!(
                "    using the PostgreSQL of {CONN_STRING_ENV}; its role must be no superuser, \
                 have NOBYPASSRLS and have CREATE on the database"
            ));
            let Some(superuser_url) = env(SUPERUSER_CONN_STRING_ENV) else {
                bail!(
                    "{CONN_STRING_ENV} is set without {SUPERUSER_CONN_STRING_ENV}: the tests of \
                     a role the policies do not hold need the URL of a superuser of the same \
                     database"
                );
            };
            containers::PostgresUrls {
                tests: url,
                superuser: superuser_url,
            }
        } else {
            containers::prepare_postgres(&mut self.uses)?
        };
        self.postgres = Some(urls.clone());
        Ok(urls)
    }

    /// The object store of the environment, or of the container prepared for the tests.
    fn object_store(&mut self) -> Result<(String, String)> {
        if let Some(store) = &self.object_store {
            return Ok(store.clone());
        }
        let store =
            if let (Some(endpoint), Some(bucket)) = (env(S3_ENDPOINT_ENV), env(S3_BUCKET_ENV)) {
                steps::say(&format!("    using the object store of {S3_ENDPOINT_ENV}"));
                (endpoint, bucket)
            } else {
                let (endpoint, bucket) = containers::prepare_object_store(&mut self.uses)?;
                (endpoint, bucket.to_owned())
            };
        self.object_store = Some(store.clone());
        Ok(store)
    }
}

/// What is wrong with the tests that `arguments` for the harness chose, which ran in
/// `results`, the units of `database`: they were none, or some of the names of a rerun
/// line (`-- --exact` and names) are of no test the harness named (run or ignored).
/// Either proves nothing. When a binary of a unit is not accounted for (it died, a
/// signal stopped it, cargo's count was not read), its tests are not all known, and
/// nothing is said of them: the unit failed.
fn chose_wrongly(
    database: Database,
    arguments: &[String],
    results: &[&UnitResult],
) -> Option<String> {
    if arguments.is_empty()
        || !results
            .iter()
            .all(|result| result.every_binary_accounted_for())
    {
        return None;
    }
    let seen = |name: &String| {
        results
            .iter()
            .any(|result| result.run.report.seen_tests.contains(name))
    };
    if results.iter().all(|result| {
        let report = &result.run.report;
        report.passed == 0 && report.failed == 0 && report.seen_tests.is_empty()
    }) {
        return Some(format!(
            "`{}` chose no test of {}",
            arguments.join(" "),
            database.name()
        ));
    }
    let unseen: Vec<&str> = exact_names(arguments)
        .unwrap_or_default()
        .iter()
        .filter(|name| !seen(name))
        .map(String::as_str)
        .collect();
    (!unseen.is_empty()).then(|| {
        format!(
            "no test of {} is named {}: a name that moved, or a rerun of another source",
            database.name(),
            unseen.join(", ")
        )
    })
}

/// The names of arguments for the harness that are `--exact` and names only, as the
/// rerun lines of the summary give them.
fn exact_names(arguments: &[String]) -> Option<&[String]> {
    match arguments {
        [exact, names @ ..]
            if exact == "--exact"
                && !names.is_empty()
                && names.iter().all(|name| !name.starts_with('-')) =>
        {
            Some(names)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn named(names: &[&str]) -> Vec<Unit> {
        let metadata = format!(
            r#"{{"packages": [{{"name": "p", "targets": [
                {{"name": "p", "kind": ["lib"], "test": true, "doctest": true}}{}]}}]}}"#,
            names.iter().fold(String::new(), |mut targets, name| {
                let _ = write!(
                    targets,
                    r#", {{"name": "{name}", "kind": ["test"], "test": true, "doctest": false}}"#
                );
                targets
            })
        );
        units::of_packages(&metadata, None, Services::Sqlite).expect("units")
    }

    #[test]
    fn a_whole_database_builds_as_cargo_test_does_and_a_part_builds_its_targets() {
        let all = named(&["api", "web"]);
        assert_eq!(
            build_arguments(Database::Sqlite, &all, &all),
            Some(
                ["test", "--locked", "--no-run", "--workspace"]
                    .map(str::to_owned)
                    .to_vec()
            ),
            "everything, examples too"
        );
        let some: Vec<Unit> = all
            .iter()
            .filter(|unit| unit.name != "api")
            .cloned()
            .collect();
        assert_eq!(
            build_arguments(Database::Postgres, &all, &some),
            Some(
                [
                    "test",
                    "--locked",
                    "--no-run",
                    "--package",
                    "fvoci-store",
                    "--package",
                    "fvoci-server",
                    "--lib",
                    "--test",
                    "web"
                ]
                .map(str::to_owned)
                .to_vec()
            ),
            "the targets of the part, with the database's packages"
        );
        let doc: Vec<Unit> = all.into_iter().filter(Unit::is_doc).collect();
        assert_eq!(
            build_arguments(Database::Sqlite, &named(&["api"]), &doc),
            None,
            "documentation tests are built as they run"
        );
        let two_units = named(&["api"]);
        let halves = shards::expand_units(
            two_units
                .iter()
                .filter(|unit| unit.name == "api")
                .cloned()
                .collect(),
            "1/2".parse().expect("part"),
        );
        assert_eq!(
            build_arguments(Database::Postgres, &two_units, &halves),
            Some(
                [
                    "test",
                    "--locked",
                    "--no-run",
                    "--package",
                    "fvoci-store",
                    "--package",
                    "fvoci-server",
                    "--test",
                    "api",
                ]
                .map(str::to_owned)
                .to_vec()
            ),
            "two subunits of api build `--test api` once"
        );
    }

    #[test]
    fn a_rerun_fails_when_it_chose_no_test_or_a_name_of_none() {
        use crate::test::harness::{Report, UnitRun};
        let words = |text: &str| -> Vec<String> { text.split(' ').map(str::to_owned).collect() };
        let ran = |unit: &str, seen: &[&str]| UnitResult {
            database: "sqlite",
            unit: unit.to_owned(),
            scope: Scope::Chosen,
            expected_tests: None,
            run: UnitRun {
                succeeded: true,
                interrupted: false,
                failed_targets: None,
                report: Report {
                    passed: u64::try_from(seen.len()).expect("a few"),
                    seen_tests: seen.iter().map(|&name| name.to_owned()).collect(),
                    ..Report::default()
                },
                elapsed: std::time::Duration::ZERO,
            },
        };
        let api = ran("api", &["api::a"]);
        let web = ran("web", &[]);
        let both = [&api, &web];
        let wrongly = |arguments: &str, results: &[&UnitResult]| {
            chose_wrongly(Database::Sqlite, &words(arguments), results)
        };
        assert_eq!(
            wrongly("--exact api::a", &both),
            None,
            "web ran none of it: no matter"
        );
        assert_eq!(wrongly("api", &both), None, "a filter that chose a test");
        assert!(
            wrongly("--exact api::a api::moved", &both)
                .is_some_and(|wrongly| wrongly.contains("named api::moved")),
            "one stale name of two"
        );
        assert!(
            wrongly("--exact api::moved --nocapture", &[&web])
                .is_some_and(|wrongly| wrongly.contains("chose no test of sqlite")),
            "a rerun line with more after it that runs nothing"
        );
        assert_eq!(
            chose_wrongly(Database::Sqlite, &[], &[&web]),
            None,
            "a whole unit"
        );
        let mut died = ran("api", &[]);
        died.run.report.started = 1;
        assert_eq!(
            wrongly("--exact api::a", &[&died, &web]),
            None,
            "a binary that died before it named its tests says nothing of the names"
        );
    }

    #[test]
    fn only_the_shape_of_a_rerun_line_gives_names_to_check() {
        let words = |text: &str| -> Vec<String> { text.split(' ').map(str::to_owned).collect() };
        assert_eq!(
            exact_names(&words("--exact api::a web::b")),
            Some(&words("api::a web::b")[..])
        );
        for other in [
            "api",
            "--exact",
            "--exact api::a --nocapture",
            "--skip x --exact a",
        ] {
            assert_eq!(exact_names(&words(other)), None, "{other}");
        }
    }

    #[test]
    fn the_s3_units_are_the_three_tests_that_need_s3() {
        let units = Database::S3.units("{}").expect("units");
        let names: Vec<&str> = units.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, ["storage", "api", "organization_boundary_round2"]);
        assert!(
            units.iter().all(|unit| unit.flags == ["--ignored"]),
            "they are ignored unless asked for"
        );
        assert_eq!(units[2].services, Services::PostgresS3);
    }

    #[test]
    fn an_unpopulated_subunit_is_refused_before_running() {
        let mut halves = shards::expand_units(named(&["api"]), "1/2".parse().expect("part"));
        let api_half = halves
            .iter_mut()
            .find(|unit| unit.slice.is_some())
            .expect("api/1");
        assert!(
            ensure_subunit_filter(api_half).is_err(),
            "empty filter on a subunit fails fast"
        );
        api_half.filter = vec!["boards::".to_owned()];
        assert!(ensure_subunit_filter(api_half).is_ok());
    }
}
