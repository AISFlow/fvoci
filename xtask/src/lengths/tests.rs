use super::measure::measure_source;
use super::*;
use crate::files::testing::{scratch, write};

const CODE: &str = "crates/a/src/lib.rs";

fn file(name: &str, lines: usize) -> Length {
    Length {
        subject: Subject::File(name.to_owned()),
        lines,
        at: 1,
    }
}

/// A function of the file [`CODE`].
fn function(name: &str, lines: usize, at: usize) -> Length {
    function_in(CODE, name, lines, at)
}

fn function_in(file: &str, name: &str, lines: usize, at: usize) -> Length {
    Length {
        subject: Subject::Function {
            file: file.to_owned(),
            name: name.to_owned(),
        },
        lines,
        at,
    }
}

/// The functions of `source`, as `name lines`.
fn functions(source: &str) -> Vec<String> {
    measure_source("planted.rs", source)
        .expect("the source parses")
        .lengths
        .into_iter()
        .filter_map(|length| match length.subject {
            Subject::Function { name, .. } => Some(format!("{name} {}", length.lines)),
            Subject::File(_) => None,
        })
        .collect()
}

/// What is refused in `source`, as the file `file`, whatever its lengths.
fn refused(file: &str, source: &str) -> Vec<String> {
    measure_source(file, source)
        .expect("the source parses")
        .refused
}

/// A function of `lines` lines.
fn long(name: &str, lines: usize) -> String {
    format!("fn {name}() {{\n{}}}\n", "    // …\n".repeat(lines - 2))
}

/// What the step finds over its limit in the workspace `root`.
fn differences(root: &Path) -> Vec<String> {
    let measured = measure_all(root).expect("the sources read");
    judge(&measured.lengths)
}

#[test]
fn what_is_at_its_limit_is_within_it() {
    let measured = [
        file(CODE, MAX_FILE_LINES),
        file("crates/a/tests/api/main.rs", MAX_TEST_FILE_LINES),
        file("crates/a/src/lengths/tests.rs", MAX_TEST_FILE_LINES),
        function("short", MAX_FUNCTION_LINES, 1),
        function("tests::a_scene", MAX_TEST_FUNCTION_LINES, 1),
        function_in(
            "crates/a/tests/api/main.rs",
            "scene",
            MAX_TEST_FUNCTION_LINES,
            1,
        ),
    ];
    assert_eq!(
        judge(&measured),
        Vec::<String>::new(),
        "at the limit is within it"
    );
}

#[test]
fn a_long_file_and_a_long_function_are_refused() {
    let measured = [
        file(CODE, 1001),
        file("crates/a/tests/api/main.rs", 2001),
        function("grown", 101, 7),
        function("tests::a_scene", 201, 900),
    ];
    assert_eq!(
        judge(&measured),
        [
            "crates/a/src/lib.rs:1: crates/a/src/lib.rs has 1001 lines, the limit is 1000",
            "crates/a/tests/api/main.rs:1: crates/a/tests/api/main.rs has 2001 lines, the limit is 2000",
            "crates/a/src/lib.rs:7: crates/a/src/lib.rs: fn grown has 101 lines, the limit is 100",
            "crates/a/src/lib.rs:900: crates/a/src/lib.rs: fn tests::a_scene has 201 lines, the limit is 200",
        ]
    );
}

#[test]
fn only_the_tests_of_a_crate_get_the_limits_of_tests() {
    for (path, of_tests) in [
        ("crates/a/tests/api/main.rs", true),
        ("xtask/tests/it.rs", true),
        ("crates/a/src/app/tests.rs", true),
        ("crates/a/src/tests/production.rs", false),
        ("crates/a/src/testsuite.rs", false),
        ("crates/a/src/app/contests.rs", false),
    ] {
        assert_eq!(is_test_file(path), of_tests, "{path}");
    }
    let limit = |file: &str, name: &str| function_in(file, name, 1, 1).subject.limit();
    assert_eq!(limit(CODE, "tests::a_scene"), MAX_TEST_FUNCTION_LINES);
    assert_eq!(limit(CODE, "contests::run"), MAX_FUNCTION_LINES);
    assert_eq!(
        limit("crates/a/tests/it.rs", "scene"),
        MAX_TEST_FUNCTION_LINES
    );
    assert_eq!(
        limit("crates/a/src/tests/production.rs", "run"),
        MAX_FUNCTION_LINES,
        "a directory named tests inside src is code"
    );
}

#[test]
fn what_an_impl_is_for_does_not_make_its_functions_tests() {
    let limit = |name: &str| function(name, 1, 1).subject.limit();
    assert_eq!(
        limit("<Planted as From<crate::support::tests::Marker>>::from"),
        MAX_FUNCTION_LINES,
        "a path inside the name of a trait is not around the function"
    );
    assert_eq!(
        limit("tests::<Planted as From<Marker>>::from"),
        MAX_TEST_FUNCTION_LINES
    );
}

#[test]
fn code_in_a_file_that_is_not_measured_is_refused() {
    let problems = refused(
        CODE,
        "include!(\"generated.rs\");\nconst TEXT: &str = include_str!(\"text.txt\");\n\
         #[path = \"big.inc\"]\nmod big;\n#[path = \"elsewhere.rs\"]\nmod fine;",
    );
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].starts_with("crates/a/src/lib.rs:1: include!")
            && problems[1].starts_with("crates/a/src/lib.rs:4: the module big is kept in big.inc"),
        "{problems:?}"
    );
    let problems = refused(
        CODE,
        "#[path = \"billing/tests.rs\"]\nmod ledger;\n#[cfg(test)]\n#[path = \"unit/tests.rs\"]\nmod tests;",
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with(
            "crates/a/src/lib.rs:2: the module ledger is kept in a file named tests.rs"
        ),
        "a file takes the limits of tests by its name only as the test module: {problems:?}"
    );
}

#[test]
fn every_member_of_the_workspace_is_measured() {
    let manifest = std::fs::read_to_string(crate::workspace_root().join("Cargo.toml"))
        .expect("the manifest is readable");
    let members = manifest
        .lines()
        .find_map(|line| line.strip_prefix("members = ["))
        .expect("the members are on one line");
    for member in members.split('"').skip(1).step_by(2) {
        assert!(
            DIRECTORIES
                .iter()
                .any(|directory| member == *directory
                    || member.starts_with(&format!("{directory}/"))),
            "the member {member} is under none of DIRECTORIES, so its files are not measured"
        );
    }
}

#[test]
fn the_name_of_the_test_module_is_kept_for_it() {
    let source = "
mod tests { fn production() {} }
#[cfg(not(test))] mod tests {}
mod inner { mod tests; }
fn tests() {}
struct tests;
impl tests { fn run(&self) {} }
trait tests {}
";
    let lines: Vec<String> = refused(CODE, source)
        .iter()
        .map(|problem| problem.split(' ').take(4).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        lines,
        [
            "crates/a/src/lib.rs:2: a module that",
            "crates/a/src/lib.rs:3: a module that",
            "crates/a/src/lib.rs:4: a module that",
            "crates/a/src/lib.rs:5: a function is",
            "crates/a/src/lib.rs:6: a type is",
            "crates/a/src/lib.rs:7: a type is",
            "crates/a/src/lib.rs:8: a trait is",
        ],
        "code cannot take the limit of tests by its name"
    );
    assert_eq!(
        refused(
            CODE,
            "#[cfg(test)] mod tests { fn a() {} }\n#[cfg(all(test, unix))] mod tests;\nmod contests {}"
        ),
        Vec::<String>::new(),
        "a test module, inline or in its file"
    );
    assert_eq!(
        refused("crates/a/tests/api/main.rs", "mod tests; fn tests() {}"),
        Vec::<String>::new(),
        "in the test directory of a crate the name decides nothing"
    );
}

#[test]
fn a_line_that_the_formatter_does_not_break_is_refused() {
    let problems = refused(
        CODE,
        "fn fine() {}\n#[rustfmt::skip]\nfn crammed() { a(); b(); c(); }\n#[cfg_attr(rustfmt, rustfmt::skip)]\nmod m {}",
    );
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].starts_with("crates/a/src/lib.rs:2: rustfmt::skip")
            && problems[1].starts_with("crates/a/src/lib.rs:4: rustfmt::skip"),
        "{problems:?}"
    );
    let error = measure_source(CODE, "fn a() {\r    b();\r}\r").expect_err("lone carriage returns");
    assert!(
        format!("{error:#}").contains("carriage return"),
        "lines that end without a line feed are not counted as one: {error:#}"
    );
    assert_eq!(
        functions("fn a() {\r\n    b();\r\n}\r\n"),
        ["a 3"],
        "a carriage return before the line feed is fine"
    );
}

#[test]
fn a_planted_long_function_is_measured_from_fn_to_its_closing_brace() {
    let source = format!(
        "/// Documented.\n#[inline]\npub(crate) async\nfn planted() {{\n{}}}\n",
        "\n    // a comment counts, and so does an empty line\n".repeat(60)
    );
    assert_eq!(functions(&source), ["planted 122"]);
    let measured = measure_source(CODE, &source).expect("the source parses");
    assert_eq!(
        judge(&measured.lengths),
        ["crates/a/src/lib.rs:4: crates/a/src/lib.rs: fn planted has 122 lines, the limit is 100"]
    );
}

#[test]
fn a_function_is_named_with_what_is_around_it() {
    let source = "
fn free() {}
struct App;
impl App {
    fn run(&self) {
        fn helper() {}
        let closure = || {
            1
        };
    }
}
impl<T> From<T> for App { fn from(_: T) -> Self { App } }
impl From<&'static str> for App { fn from(_: &'static str) -> Self { App } }
impl std::fmt::Display for &App { fn fmt(&self) {} }
impl Tuple for (u8, u8) { fn first(&self) {} }
trait Shape {
    fn area(&self) -> u32;
    fn doubled(&self) -> u32 {
        self.area() * 2
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn r#move() {}
}
";
    assert_eq!(
        functions(source),
        [
            "free 1",
            "App::run 6",
            "App::run::helper 1",
            "<App as From<T>>::from 1",
            "<App as From<&'staticstr>>::from 1",
            "<App as std::fmt::Display>::fmt 1",
            "<_ as Tuple>::first 1",
            "Shape::doubled 3",
            "tests::move 1",
        ],
        "a closure is part of its function; a declaration without a body is no function; \
         the conversions from two types are two names"
    );
}

#[test]
fn a_file_is_measured_in_lines() {
    let lines = |source| {
        measure_source("a.rs", source)
            .expect("the source parses")
            .lengths
            .first()
            .map(|length| length.lines)
    };
    assert_eq!(lines(""), Some(0));
    assert_eq!(lines("fn a() {}\n"), Some(1));
    assert_eq!(
        lines("fn a() {}\n\nfn b() {}"),
        Some(3),
        "without a last line break"
    );
    assert!(
        measure_source("a.rs", "fn (").is_err(),
        "a file that does not parse is not passed over"
    );
}

#[test]
fn planted_long_code_is_found_in_a_workspace() {
    let root = scratch("lengths-workspace");
    write(&root, "crates/a/src/lib.rs", &long("planted", 101));
    write(&root, "crates/a/src/long.rs", &"// a line\n".repeat(1001));
    write(
        &root,
        "crates/a/tests/it.rs",
        &format!("{}{}", long("scene", 200), "// a line\n".repeat(801)),
    );
    write(&root, "crates/a/assets/data.txt", &"a line\n".repeat(5000));
    write(&root, "xtask/src/main.rs", "fn main() {}\n");

    assert_eq!(
        differences(&root),
        [
            "crates/a/src/long.rs:1: crates/a/src/long.rs has 1001 lines, the limit is 1000",
            "crates/a/src/lib.rs:1: crates/a/src/lib.rs: fn planted has 101 lines, the limit is 100",
        ],
        "a test file and a test of that length are within their limits, and only Rust is measured"
    );
    let error = run(&root).expect_err("the step fails for what is over its limit");
    assert_eq!(
        error.to_string(),
        "2 files or functions are over their limit"
    );

    write(&root, "crates/a/src/long.rs", &"// a line\n".repeat(1000));
    write(&root, "crates/a/src/lib.rs", &long("planted", 100));
    run(&root).expect("the step passes once both are within their limits");

    write(
        &root,
        "crates/a/src/lib.rs",
        "#[rustfmt::skip]\nfn crammed() {}\n",
    );
    run(&root).expect_err("what is refused whatever its length fails the step");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_among_the_sources_is_refused() {
    let root = scratch("lengths-link");
    write(&root, "crates/a/src/lib.rs", "fn a() {}\n");
    std::os::unix::fs::symlink(
        root.join("crates/a/src/lib.rs"),
        root.join("crates/a/src/linked.rs"),
    )
    .expect("the link is made");
    let error = files::rust_files(&root.join("crates")).expect_err("a link is not followed");
    assert!(
        error.to_string().ends_with("linked.rs is a symbolic link"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
