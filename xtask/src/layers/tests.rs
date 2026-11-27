use super::scan::read_source;
use super::walk::walk_crate;
use super::*;
use crate::files::testing::{scratch, write};

/// The sources of a workspace made of `files`: each a module and its source.
fn sources(files: &[(&str, &str)]) -> Sources {
    let mut sources = Sources::default();
    for (module, source) in files {
        let path: Vec<String> = module.split("::").map(str::to_owned).collect();
        sources
            .add(&format!("{module}.rs"), &path, source)
            .expect("the source parses");
    }
    sources
}

/// What the module `module` names against the rule in a workspace made of `files`:
/// `from>to named`.
fn broken_in(files: &[(&str, &str)], module: &str) -> Vec<String> {
    sources(files)
        .violations()
        .iter()
        .filter(|violation| violation.file == format!("{module}.rs"))
        .map(|violation| format!("{}>{} {}", violation.from, violation.to, violation.named))
        .collect()
}

/// What `source`, as the module `module`, names against the rule: `from>to named`.
fn broken(module: &str, source: &str) -> Vec<String> {
    broken_in(&[(module, source)], module)
}

/// The paths of two names or more that `source` names, resolved, as the module `module`.
fn paths(module: &str, source: &str) -> Vec<String> {
    let module: Vec<String> = module.split("::").map(str::to_owned).collect();
    read_source(&module, source, &[])
        .expect("the source parses")
        .references
        .into_iter()
        .map(|Reference { target, .. }| match target {
            Target::Path { path, .. } => path,
            Target::Handle { written, .. } => written,
            Target::Included => "include!".to_owned(),
        })
        .filter(|path| path.contains("::"))
        .collect()
}

#[test]
fn the_table_of_layers_is_consistent() {
    let mut owned: Vec<&str> = Vec::new();
    for layer in LAYERS {
        for used in layer.may_use {
            assert!(
                LAYERS.iter().any(|other| other.name == *used),
                "{} may use {used}, which is no layer",
                layer.name
            );
        }
        owned.extend(layer.owns);
    }
    let count = owned.len();
    owned.sort_unstable();
    owned.dedup();
    assert_eq!(owned.len(), count, "no path is claimed twice");
    for (name, _) in CRATES {
        assert!(layer_of(name).is_some(), "the crate {name} is in a layer");
    }
}

#[test]
fn a_path_belongs_to_the_layer_with_the_longest_claim() {
    let name = |path| layer_of(path).map(|layer| layer.name);
    assert_eq!(name("fvoci_server"), Some("http"), "the crate root");
    assert_eq!(name("fvoci_server::api::users"), Some("http"));
    assert_eq!(name("fvoci_server::brand_new"), Some("http"), "the default");
    assert_eq!(name("fvoci_server::app"), Some("logic"));
    assert_eq!(name("fvoci_server::app::boards::tests"), Some("logic"));
    assert_eq!(
        name("fvoci_server::application"),
        Some("http"),
        "a claim ends at a whole name"
    );
    assert_eq!(name("fvoci_store::Store"), Some("store"));
    assert_eq!(name("axum::http::StatusCode"), Some("http"));
    assert_eq!(name("fvoci_server::metrics::requests"), Some("metrics"));
    assert_eq!(name("prometheus::Registry"), Some("metrics"));
    assert_eq!(
        name("fvoci_server::metrics_listener"),
        Some("http"),
        "serving the metrics is HTTP"
    );
    assert_eq!(name("tokio::sync::Mutex"), None, "in no layer");
}

#[test]
fn http_code_that_names_the_store_is_found() {
    assert_eq!(
        broken("fvoci_server::api::planted", "use fvoci_store::Store;"),
        ["http>store fvoci_store::Store"]
    );
    assert_eq!(
        broken(
            "fvoci_server",
            "fn open() { let _ = fvoci_store::Store::connect(); }"
        ),
        ["http>store fvoci_store::Store::connect"],
        "the crate root is HTTP"
    );
}

#[test]
fn another_name_does_not_hide_an_import() {
    let module = "fvoci_server::api::planted";
    assert_eq!(
        broken(module, "use fvoci_store as database;"),
        ["http>store fvoci_store"]
    );
    assert_eq!(
        broken(module, "use fvoci_store::Store as Database;"),
        ["http>store fvoci_store::Store"]
    );
    assert_eq!(
        broken(module, "extern crate fvoci_store as database;"),
        ["http>store fvoci_store"]
    );
    assert_eq!(
        broken(
            module,
            "use fvoci_store::{self, boards::{Board as B, *}, Store as _};"
        ),
        [
            "http>store fvoci_store",
            "http>store fvoci_store::boards::Board",
            "http>store fvoci_store::boards::*",
            "http>store fvoci_store::Store",
        ]
    );
    assert_eq!(
        broken(module, "fn f() { use fvoci_store::Store as S; }"),
        ["http>store fvoci_store::Store"],
        "an import inside a function"
    );
}

#[test]
fn a_full_path_is_found_wherever_it_is_written() {
    let module = "fvoci_server::ws";
    let found = |source| broken(module, source);
    assert_eq!(
        found("async fn f() { fvoci_store::Store::connect().await; }"),
        ["http>store fvoci_store::Store::connect"],
        "an expression"
    );
    assert_eq!(
        found("fn f(store: &::fvoci_store::Store) {}"),
        ["http>store fvoci_store::Store"],
        "a type, from the root of all crates"
    );
    assert_eq!(
        found("struct S { all: Vec<Option<fvoci_store::Store>> }"),
        ["http>store fvoci_store::Store"],
        "inside generic arguments"
    );
    assert_eq!(
        found("fn f(e: E) -> bool { matches!(e, fvoci_store::StoreError::NotFound) }"),
        ["http>store fvoci_store::StoreError::NotFound"],
        "the input of a macro"
    );
    assert_eq!(
        found("fn f() { tokio::select! { x = ::fvoci_store::connect() => {} } }"),
        ["http>store fvoci_store::connect"],
        "the input of a macro, from the root of all crates"
    );
    assert_eq!(
        found(
            "#[utoipa::path(get, responses((status = 200, body = fvoci_store::Workspace)))] fn f() {}"
        ),
        ["http>store fvoci_store::Workspace"],
        "the arguments of an attribute"
    );
    assert_eq!(
        found(
            "impl From<fvoci_store::StoreError> for E { fn from(e: fvoci_store::StoreError) -> Self { E } }"
        ),
        [
            "http>store fvoci_store::StoreError",
            "http>store fvoci_store::StoreError"
        ],
        "a trait and a parameter"
    );
    assert_eq!(
        found("#[cfg(test)] mod tests { use fvoci_store::testing::memory; }"),
        ["http>store fvoci_store::testing::memory"],
        "a test module is checked like the code around it"
    );
    assert_eq!(
        found("macro_rules! open { () => { $crate::app::App::open(fvoci_store::Store::new()) }; }"),
        ["http>store fvoci_store::Store::new"],
        "the body of a macro definition"
    );
}

#[test]
fn a_handle_of_the_store_is_found() {
    let module = "fvoci_server::api::planted";
    let found = |source| broken(module, source);
    assert_eq!(
        found("async fn f(app: App) { app.store().get_block(1).await; }"),
        ["http>store .store().get_block"]
    );
    assert_eq!(
        found("async fn f(hub: Hub) { hub.inner.store.get_block(1).await; }"),
        ["http>store .store.get_block"],
        "a field"
    );
    assert_eq!(
        found("fn f(app: App) { let store = app.store(); pass(&app.inner.store); }"),
        ["http>store .store()", "http>store .store"],
        "taken without a call on it"
    );
    assert_eq!(
        found("async fn f(app: App) { tokio::join!(app.store().a(), other()); }"),
        ["http>store .store().a"],
        "inside a macro"
    );
    assert!(
        found("fn f(seen: AtomicBool, a: A) { seen.store(true, SeqCst); a.restore(); let store = 1; let _ = 0..store; }")
            .is_empty(),
        "a method with arguments, another name, a local and a range are not the handle"
    );
}

#[test]
fn a_name_given_elsewhere_does_not_hide_what_it_stands_for() {
    let logic = (
        "fvoci_server::app",
        "mod boards; pub use boards::*; pub use fvoci_store::Store as Database; \
         pub use fvoci_store::PublicBoards; pub type Shared = std::sync::Arc<Database>; \
         pub struct App;",
    );
    let boards = (
        "fvoci_server::app::boards",
        "pub use fvoci_store::Workspace;",
    );
    let found = |source| {
        broken_in(
            &[logic, boards, ("fvoci_server::api::planted", source)],
            "fvoci_server::api::planted",
        )
    };
    assert_eq!(
        found("use crate::app::{App, Database, PublicBoards};"),
        [
            "http>store fvoci_store::Store",
            "http>store fvoci_store::PublicBoards"
        ],
        "a re-export, also under another name; the logic's own type is fine"
    );
    assert_eq!(
        found("fn f(shared: crate::app::Shared) {}"),
        ["http>store fvoci_store::Store"],
        "a type alias"
    );
    assert_eq!(
        found("use crate::app as logic; fn f() { logic::Database::connect(); }"),
        ["http>store fvoci_store::Store::connect"],
        "through a module that was given another name"
    );
    assert_eq!(
        found("use crate::app::Workspace;"),
        ["http>store fvoci_store::Workspace"],
        "through a module that re-exports everything of another"
    );
    assert_eq!(
        found(
            "use fvoci_store as database; fn f() { database::Store::connect(); database::open(); }"
        ),
        [
            "http>store fvoci_store",
            "http>store fvoci_store::Store::connect",
            "http>store fvoci_store::open"
        ],
        "an import and every path that starts at the name it gave"
    );
    assert!(
        broken_in(&[logic, boards], "fvoci_server::app").is_empty(),
        "the logic may give those names"
    );
}

#[test]
fn a_function_that_has_the_name_of_its_module_is_followed() {
    let logic = (
        "fvoci_server::app",
        "mod download; pub(crate) use download::download; pub(crate) use download::Slot;",
    );
    let download = (
        "fvoci_server::app::download",
        "use super::Slot as Place; pub(crate) struct Slot; \
         pub(crate) fn download(place: Place) -> fvoci_store::Store { todo!() }",
    );
    let handler = (
        "fvoci_server::api::planted",
        "use crate::app::{Slot, download}; fn f(slot: Slot) { download(slot); }",
    );
    let sources = sources(&[logic, download, handler]);
    assert!(
        sources.refused().is_empty(),
        "the module and the function share a name and are no circle: {:?}",
        sources.refused()
    );
    assert!(
        sources.violations().is_empty(),
        "logic, named by HTTP and by itself: {:?}",
        sources.violations()
    );
    assert_eq!(
        sources.reached("fvoci_server::app::download"),
        [
            "fvoci_server::app::download",
            "fvoci_server::app::download::download"
        ],
        "the name alone is the module or the function"
    );
    assert_eq!(
        sources.reached("fvoci_server::app::download::Slot"),
        ["fvoci_server::app::download::Slot"],
        "before `::` it is the module"
    );
}

#[test]
fn a_module_of_the_same_name_does_not_hide_what_a_name_stands_for() {
    let found = broken_in(
        &[
            (
                "fvoci_server::api",
                "mod open; mod planted; pub(crate) use fvoci_store::open; \
                 mod inner { pub(crate) mod connect {} pub(crate) use fvoci_store::connect; }",
            ),
            ("fvoci_server::api::open", "pub(crate) struct Options;"),
            (
                "fvoci_server::api::planted",
                "use super::open; use super::inner::connect; \
                 fn f(options: open::Options) { open(options); connect(); }",
            ),
        ],
        "fvoci_server::api::planted",
    );
    assert_eq!(
        found,
        [
            "http>store fvoci_store::open",
            "http>store fvoci_store::connect",
            "http>store fvoci_store::open",
            "http>store fvoci_store::connect"
        ],
        "the import and the call of each, beside a module in a file and one written inline; \
         `open::Options` is the module's"
    );

    let found = broken_in(
        &[
            (
                "fvoci_server::app",
                "mod download; pub(crate) use download::download;",
            ),
            (
                "fvoci_server::app::download",
                "pub(crate) use fvoci_store::Store as Slot; pub(crate) fn download() {}",
            ),
            (
                "fvoci_server::api::planted",
                "use crate::app::download; \
                 fn f(a: download::Slot, b: crate::app::download::Slot) { download(); }",
            ),
        ],
        "fvoci_server::api::planted",
    );
    assert_eq!(
        found, ["http>store fvoci_store::Store"; 2],
        "what the module holds is followed through its name, short and in full"
    );
}

#[test]
fn a_name_is_the_module_only_where_the_module_is_there_everywhere() {
    let handler = (
        "fvoci_server::api::planted",
        "fn f(store: crate::app::db::Store) { crate::app::db::connect(); }",
    );
    let found = |logic: &[(&'static str, &'static str)]| {
        let mut files = logic.to_vec();
        files.push(handler);
        broken_in(&files, "fvoci_server::api::planted")
    };
    let both = [
        "http>store fvoci_store::Store",
        "http>store fvoci_store::connect",
    ];
    assert_eq!(
        found(&[
            (
                "fvoci_server::app",
                "#[cfg(test)] mod db; #[cfg(not(test))] pub(crate) use fvoci_store as db;"
            ),
            ("fvoci_server::app::db", ""),
        ]),
        both,
        "a module under a condition, and the name given where the module is not"
    );
    assert_eq!(
        found(&[(
            "fvoci_server::app",
            "#[cfg(test)] mod db {} #[cfg(not(test))] pub(crate) use fvoci_store as db;"
        )]),
        both,
        "the same, written inline"
    );
    assert_eq!(
        found(&[
            (
                "fvoci_server::app",
                "#[cfg(unix)] mod inner; #[cfg(not(unix))] #[path = \"other.rs\"] mod inner; \
                 pub(crate) use inner::db;"
            ),
            ("fvoci_server::app::inner", "pub(crate) mod db;"),
            ("fvoci_server::app::inner::db", ""),
            (
                "fvoci_server::app::inner",
                "pub(crate) use fvoci_store as db;"
            ),
        ]),
        both,
        "a module inside a module that is one file or another by a condition"
    );
    assert_eq!(
        found(&[(
            "fvoci_server::app",
            "pub(crate) use fvoci_store as db; fn helper() { mod db { pub(crate) fn connect() {} } }"
        )]),
        both,
        "a module written inside the body of a function is a module there only"
    );
    assert_eq!(
        broken(
            "fvoci_server::api::planted",
            "mod open; fn f() { use fvoci_store as open; open::connect(); let _: open::Store; }"
        ),
        [
            "http>store fvoci_store",
            "http>store fvoci_store::connect",
            "http>store fvoci_store::Store"
        ],
        "a name given inside the body of a function, where a module has it outside"
    );
}

#[test]
fn a_module_that_exports_everything_of_a_module_of_its_own_is_no_circle() {
    let sources = sources(&[
        ("fvoci_server::app", "mod boards; pub use boards::*;"),
        (
            "fvoci_server::app::boards",
            "pub use fvoci_store::Workspace; pub struct Board;",
        ),
        (
            "fvoci_server::api::planted",
            "use crate::app::{Board, Workspace}; fn f(board: crate::app::boards::Board) {}",
        ),
    ]);
    assert!(
        sources.refused().is_empty(),
        "the module comes before what the import of everything brought: {:?}",
        sources.refused()
    );
    let found: Vec<String> = sources
        .violations()
        .into_iter()
        .map(|violation| violation.named)
        .collect();
    assert_eq!(
        found,
        ["fvoci_store::Workspace"],
        "and what the module hands on from the store is found through it"
    );
}

#[test]
fn an_import_of_everything_of_another_layer_is_refused() {
    let sources = sources(&[
        (
            "fvoci_server::app::boards",
            "use fvoci_store::*;\nuse std::fmt::*;\nuse crate::app::*;\nmod tests { use super::*; }",
        ),
        ("fvoci_store::boards", "use fvoci_model::*;"),
    ]);
    assert_eq!(
        sources.refused(),
        [
            "fvoci_server::app::boards.rs:1: logic imports everything of store: fvoci_store::*. \
             Name what is imported",
            "fvoci_store::boards.rs:1: store imports everything of model: fvoci_model::*. \
             Name what is imported",
        ],
        "allowed or not; a layer's own modules and what is in no layer are not refused"
    );
}

#[test]
fn a_name_that_an_import_of_everything_brought_is_followed() {
    let files = [
        (
            "fvoci_server::api",
            "mod planted; use fvoci_store::DuplicateAs; use crate::app::App;",
        ),
        (
            "fvoci_server::api::planted",
            "use super::*; fn f(app: App, how: DuplicateAs) { let _ = DuplicateAs::Copy; let local = 1; g(local); }",
        ),
    ];
    assert_eq!(
        broken_in(&files, "fvoci_server::api::planted"),
        [
            "http>store fvoci_store::DuplicateAs",
            "http>store fvoci_store::DuplicateAs::Copy"
        ],
        "the name alone and a path that starts at it; the logic's type and a local are fine"
    );
}

#[test]
fn an_imported_name_counts_wherever_it_is_used_alone() {
    assert_eq!(
        broken(
            "fvoci_server::ws",
            "use fvoci_store::Store; struct Hub { store: Store } fn g(store: &Store) -> Option<Store> { None }"
        ),
        ["http>store fvoci_store::Store"; 4],
        "the import and three uses: a new parameter of that type is a new use"
    );
    assert_eq!(
        broken(
            "fvoci_server::app::planted",
            "use axum::http::StatusCode; fn f(status: StatusCode) { m!(StatusCode, other); }"
        ),
        ["logic>http axum::http::StatusCode"; 3],
        "also among the tokens of a macro"
    );
}

#[test]
fn a_path_among_the_tokens_of_a_macro_is_found_in_every_shape() {
    let found = |source| broken("fvoci_server::api::planted", source);
    assert_eq!(
        found("fn f() { tokio::select! { x = a() => ::fvoci_store::connect(), } }"),
        ["http>store fvoci_store::connect"],
        "after the arrow of an arm"
    );
    assert_eq!(
        found("fn f(n: usize) { assert!(n > ::fvoci_store::LIMIT); }"),
        ["http>store fvoci_store::LIMIT"],
        "after a comparison"
    );
    assert_eq!(
        found("fn f(x: u8) { m!(x as ::fvoci_store::Id, return ::fvoci_store::open()); }"),
        ["http>store fvoci_store::Id", "http>store fvoci_store::open"],
        "after a keyword"
    );
    assert_eq!(
        found(
            "fn f() { vec![0..fvoci_store::LIMIT]; m!(A { x: 1, ..fvoci_store::Options::default() }); }"
        ),
        [
            "http>store fvoci_store::LIMIT",
            "http>store fvoci_store::Options::default"
        ],
        "after the dots of a range"
    );
    assert_eq!(
        found(
            "cfg_if::cfg_if! { if #[cfg(unix)] { use fvoci_store::{Store, boards::{Board as B, self}, sharing::*}; \
             use fvoci_store as database; fn f(b: B) {} } }"
        ),
        [
            "http>store fvoci_store::Store",
            "http>store fvoci_store::boards::Board",
            "http>store fvoci_store::boards",
            "http>store fvoci_store::sharing::*",
            "http>store fvoci_store",
            "http>store fvoci_store::boards::Board",
        ],
        "an import with braces, and one of a crate alone; the name it gives counts after it"
    );
    assert_eq!(
        broken(
            "fvoci_server::app::planted",
            "macro_rules! handler { ($n:ident) => { fn $n() -> impl ::axum::response::IntoResponse { \
             <T as Trait>::name::call(Vec::<u8>::new()) } }; }"
        ),
        ["logic>http axum::response::IntoResponse"],
        "after the arrow of a result and after `impl`; what goes on from `>::` leads nowhere"
    );
}

#[test]
fn a_path_written_as_the_text_of_an_attribute_is_found() {
    assert_eq!(
        broken(
            "fvoci_server::api::planted",
            "#[derive(Deserialize)] struct S { #[serde(with = \"fvoci_store::serde_helpers\", rename = \"a::b\")] x: u8 } \
             fn f() { tracing::info!(\"fvoci_store::Store\"); }"
        ),
        ["http>store fvoci_store::serde_helpers"],
        "the text of a message is no path"
    );
}

#[test]
fn the_handle_of_the_store_is_found_by_the_path_of_its_accessor() {
    assert_eq!(
        broken(
            "fvoci_server::api::planted",
            "use crate::app::App; async fn f(app: App, apps: Option<&App>) { \
             App::store(&app).get_block(1).await; apps.map(App::store); }"
        ),
        ["http>store App::store"; 2]
    );
    assert!(
        broken(
            "fvoci_server::app::planted",
            "fn f(app: &App) { App::store(app); }"
        )
        .is_empty(),
        "the logic may take it"
    );
}

#[test]
fn self_and_super_lead_where_the_language_says() {
    let found = |source| broken("fvoci_server::app::planted", source);
    assert_eq!(
        found("fn f() { self::super::super::ws::Hub::new(); }"),
        ["logic>http fvoci_server::ws::Hub::new"]
    );
    assert_eq!(
        found("extern crate self as server; fn f() { server::ws::Hub::new(); }"),
        [
            "logic>http fvoci_server",
            "logic>http fvoci_server::ws::Hub::new"
        ],
        "the crate under a name of its own"
    );
}

#[test]
fn what_the_reading_cannot_follow_is_refused() {
    let sources = sources(&[
        (
            "fvoci_server::api::planted",
            "include!(\"handlers.inc\");\nconst TEXT: &str = include_str!(\"text.txt\");",
        ),
        (
            "fvoci_server::app::planted",
            "use b::x as a;\nuse a::y as b;",
        ),
    ]);
    let refused = sources.refused();
    assert_eq!(
        refused.first().map(String::as_str),
        Some(
            "fvoci_server::api::planted.rs:1: include! brings in code that is not read. Make it a module"
        ),
        "code that is included, not a text: {refused:?}"
    );
    assert!(
        refused[1..]
            .iter()
            .all(|problem| problem.contains("leads through names that are given in a circle"))
            && refused.len() > 1,
        "names that lead to each other without end: {refused:?}"
    );
}

#[test]
fn every_dependency_of_the_read_crates_is_placed() {
    assert_eq!(
        undecided_dependencies(&crate::workspace_root()).expect("the manifests read"),
        Vec::<String>::new()
    );
    for name in NO_LAYER {
        assert!(
            layer_of(name).is_none(),
            "{name} is in NO_LAYER and claimed by a layer"
        );
    }
}

#[test]
fn an_entry_of_the_table_that_is_no_module_is_noticed() {
    let modules: Vec<(&str, &str)> = LAYERS
        .iter()
        .flat_map(|layer| layer.owns)
        .filter(|owned| owned.starts_with("fvoci_server::"))
        .map(|module| (*module, ""))
        .collect();
    assert_eq!(
        sources(&modules).stale_layers(),
        Vec::<String>::new(),
        "every module of the table is there"
    );
    assert_eq!(
        sources(&modules[1..]).stale_layers(),
        [
            "fvoci_server::app is listed as logic and is no module: correct the table in \
          xtask/src/layers.rs"
        ],
        "a module that was renamed or removed"
    );
    let inline = |root: &'static str| {
        let mut files = modules[1..].to_vec();
        files.push(("fvoci_server", root));
        sources(&files).stale_layers().len()
    };
    assert_eq!(inline("mod app {}"), 0, "a module written inside a file");
    assert_eq!(
        inline("fn f() { mod app {} }"),
        1,
        "a module written inside the body of a function is no module of the crate"
    );
}

#[test]
fn the_crates_that_are_read_are_the_members_of_the_workspace() {
    let root = crate::workspace_root();
    let manifest =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("the manifest is readable");
    let members = manifest
        .lines()
        .find_map(|line| line.strip_prefix("members = ["))
        .expect("the members are on one line");
    let mut expected: Vec<String> = members
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|member| *member != "xtask")
        .map(|member| format!("{member}/src"))
        .collect();
    expected.sort();
    let mut read: Vec<&str> = CRATES.iter().map(|(_, directory)| *directory).collect();
    read.sort_unstable();
    assert_eq!(
        read, expected,
        "a crate that is added to the workspace is added to CRATES and to a layer"
    );
}

#[test]
fn logic_that_names_http_is_found() {
    let module = "fvoci_server::app::planted";
    let found = |source| broken(module, source);
    assert_eq!(
        found("use axum::http::StatusCode;"),
        ["logic>http axum::http::StatusCode"]
    );
    assert_eq!(
        found("fn f() -> http::StatusCode { http::StatusCode::OK }"),
        [
            "logic>http http::StatusCode",
            "logic>http http::StatusCode::OK"
        ]
    );
    assert_eq!(
        found("use crate::{web::WebRoot, ws::Hub, error::AppError};"),
        [
            "logic>http fvoci_server::web::WebRoot",
            "logic>http fvoci_server::ws::Hub"
        ],
        "modules of the server that are HTTP; the error type is logic"
    );
    assert_eq!(
        found("fn f() { super::super::api::created(); crate::listen_address(); }"),
        [
            "logic>http fvoci_server::api::created",
            "logic>http fvoci_server::listen_address"
        ],
        "through super, and an item of the crate root"
    );
    assert_eq!(
        found("mod tests { use super::super::super::request_log::Peer; }"),
        ["logic>http fvoci_server::request_log::Peer"],
        "super counts from the inline module"
    );
}

#[test]
fn what_the_rule_allows_is_not_reported() {
    assert!(
        broken(
            "fvoci_server::app::boards",
            "use fvoci_store::{Store, StoreError}; use fvoci_model::Board; use crate::auth::Authenticator; \
             pub(crate) fn f(app: &App) { app.store().get_board(); let http = 1; g(http); Self::h(); }"
        )
        .is_empty(),
        "logic uses the store, the model and itself; `pub(crate)` and a local are no path"
    );
    assert!(
        broken(
            "fvoci_server::api::boards",
            "use axum::Json; use fvoci_model::Board; use crate::app::App; use crate::error::AppError; \
             use std::sync::Arc; use tokio::sync::Mutex;"
        )
        .is_empty(),
        "http uses the logic, the model and what is in no layer"
    );
    assert!(
        broken(
            "fvoci_store::boards",
            "use fvoci_model::Board; use crate::db::Db; use sqlx::Row;"
        )
        .is_empty(),
        "the store uses the model"
    );
    assert_eq!(
        broken(
            "fvoci_store::boards",
            "use axum::Json; use fvoci_server::app::App;"
        ),
        [
            "store>http axum::Json",
            "store>logic fvoci_server::app::App"
        ]
    );
    assert_eq!(
        broken("fvoci_model::board", "use fvoci_store::Store;"),
        ["model>store fvoci_store::Store"]
    );
}

#[test]
fn the_metrics_are_counted_into_and_name_no_other_layer() {
    let counting = "use crate::metrics::{LoginLimit, Metrics}; \
                    fn f(metrics: &Metrics) { metrics.login_refused(LoginLimit::AddressBusy); }";
    assert!(
        broken("fvoci_server::auth::limiter", counting).is_empty(),
        "the logic counts"
    );
    assert!(
        broken("fvoci_server::ws", counting).is_empty(),
        "HTTP counts"
    );
    assert_eq!(
        broken(
            "fvoci_server::metrics::storage",
            "use prometheus::IntGauge; use axum::response::Response; use crate::app::App; \
             async fn read(app: &App) { app.store().pool_status(); }"
        ),
        [
            "metrics>http axum::response::Response",
            "metrics>logic fvoci_server::app::App",
            "metrics>logic fvoci_server::app::App",
            "metrics>store .store().pool_status"
        ],
        "the metrics answer no request and ask neither the logic nor the database"
    );
    assert_eq!(
        broken("fvoci_store::boards", "use prometheus::IntGauge;"),
        ["store>metrics prometheus::IntGauge"],
        "the store does not count: what is measured of it, the logic reads"
    );
}

#[test]
fn paths_are_resolved_from_where_they_are_written() {
    assert_eq!(
        paths(
            "fvoci_server::auth",
            "mod token; use token::Origins; fn f() { token::read(); self::token::write(); }"
        ),
        [
            "fvoci_server::auth::token::Origins",
            "fvoci_server::auth::token::read",
            "fvoci_server::auth::token::write"
        ],
        "a module declared in the file"
    );
    assert_eq!(
        paths(
            "fvoci_server::auth",
            "mod token; mod tests { use super::*; fn f() { token::read(); } }"
        ),
        ["fvoci_server::auth::*", "fvoci_server::auth::token::read"],
        "a module declared around an inline module"
    );
    assert_eq!(
        paths("fvoci_server::main", "use crate::config::Configuration;"),
        ["fvoci_server::config::Configuration"],
        "the program's paths lead into its library"
    );
    assert_eq!(
        paths(
            "fvoci_server::api",
            "fn f() { <Vec<u8> as std::io::Write>::flush(); Vec::<a::B>::new(); x.y::<a::C>(); super::super::super::gone(); }"
        ),
        ["std::io::Write::flush", "Vec::new", "a::B", "a::C"],
        "a qualified path, generic arguments, and more `super` than there are modules"
    );
    assert_eq!(
        paths(
            "a",
            "fn f() { m!(<T as b::Trait>::name(), Vec::<u8>::new(), r#mod::x, .5); }"
        ),
        ["b::Trait", "mod::x"],
        "among the tokens of a macro, what follows `>::` goes on from before"
    );
}

#[test]
fn each_use_that_breaks_the_rule_is_said_once_with_its_lines() {
    let violation = |file: &str, line, named: &str| Violation {
        file: file.to_owned(),
        line,
        from: "http",
        to: "store",
        named: named.to_owned(),
    };
    let found = [
        violation("a.rs", 3, "fvoci_store::Store"),
        violation("a.rs", 9, "fvoci_store::Store"),
        violation("b.rs", 1, ".store().close"),
    ];
    assert_eq!(
        judge(&found),
        [
            "a.rs:3,9: http names store: fvoci_store::Store",
            "b.rs:1: http names store: .store().close",
        ]
    );
    assert_eq!(judge(&[]), Vec::<String>::new(), "nothing breaks the rule");
}

/// The modules of the crate `k` in `directory`, each with its file.
fn walked(directory: &Path) -> Result<Vec<String>> {
    let mut seen = Vec::new();
    walk_crate("k", directory, &mut |module_file| {
        let source = std::fs::read_to_string(&module_file.file)?;
        seen.push(format!(
            "{} {}",
            module_file.module.join("::"),
            crate::files::shown_path(directory, &module_file.file)
        ));
        Ok(read_source(&module_file.module, &source, &[])?.declared)
    })?;
    seen.sort();
    Ok(seen)
}

#[test]
fn the_modules_of_a_crate_are_found_along_their_declarations() {
    let root = scratch("layers-walk");
    write(
        &root,
        "lib.rs",
        "mod a; mod b; #[path = \"elsewhere.rs\"] mod d; mod e { mod f; }",
    );
    write(&root, "main.rs", "");
    write(&root, "a.rs", "mod inner;");
    write(&root, "a/inner.rs", "");
    write(&root, "b/mod.rs", "mod c;");
    write(&root, "b/c.rs", "");
    write(&root, "elsewhere.rs", "");
    write(&root, "e/f.rs", "");

    let seen = walked(&root).expect("every file is a module");
    assert_eq!(
        seen,
        [
            "k lib.rs",
            "k::a a.rs",
            "k::a::inner a/inner.rs",
            "k::b b/mod.rs",
            "k::b::c b/c.rs",
            "k::d elsewhere.rs",
            "k::e::f e/f.rs",
            "k::main main.rs",
        ]
    );

    write(&root, "b/forgotten.rs", "use fvoci_store::Store;");
    let error = walked(&root).expect_err("a file that is no module");
    assert!(
        error.to_string().contains("forgotten.rs"),
        "a file the walk does not reach is not passed over: {error}"
    );

    std::fs::remove_file(root.join("b/forgotten.rs")).expect("the file is removed");
    write(
        &root,
        "a.rs",
        "mod inner; #[path = \"elsewhere.rs\"] mod again;",
    );
    let error = walked(&root).expect_err("a file that is two modules");
    assert!(
        error.to_string().contains("declared as a module twice"),
        "a file is checked as one module, so it is one: {error}"
    );
    write(&root, "a.rs", "mod inner;");

    std::fs::remove_file(root.join("b/c.rs")).expect("the file is removed");
    let error = walked(&root).expect_err("a module without a file");
    assert!(
        error.to_string().contains("k::b::c"),
        "a declared module must have a file: {error}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A workspace with two planted violations: the HTTP layer calls the store, and the
/// logic names a type of axum.
fn planted_workspace(name: &str) -> std::path::PathBuf {
    let root = scratch(name);
    write(
        &root,
        "crates/server/src/lib.rs",
        "pub mod api; pub mod app;",
    );
    write(
        &root,
        "crates/server/src/api/mod.rs",
        "use crate::app::App;\nasync fn handler(app: App) {\n    app.store().purge().await;\n}\n",
    );
    write(
        &root,
        "crates/server/src/app.rs",
        "use fvoci_store::Store;\nuse axum::response::Response;\n",
    );
    write(&root, "crates/store/src/lib.rs", "use fvoci_model::Board;");
    write(&root, "crates/collab/src/lib.rs", "use fvoci_model::Board;");
    write(&root, "crates/model/src/lib.rs", "");
    root
}

#[test]
fn a_planted_violation_is_found_in_a_workspace() {
    let root = planted_workspace("layers-workspace");
    let found: Vec<String> = Sources::read(&root)
        .expect("the sources read")
        .violations()
        .iter()
        .map(|violation| format!("{}:{} {}", violation.file, violation.line, violation.named))
        .collect();
    assert_eq!(
        found,
        [
            "crates/server/src/api/mod.rs:3 .store().purge",
            "crates/server/src/app.rs:2 axum::response::Response",
        ]
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_step_fails_a_workspace_for_any_use_that_breaks_the_rule() {
    let root = planted_workspace("layers-step");
    for (member, dependencies) in [
        (
            "server",
            "fvoci-store.workspace = true\naxum = \"1\"\ntokio = { version = \"1\" }",
        ),
        ("store", "sqlx.workspace = true"),
        ("collab", "yrs.workspace = true"),
        ("model", ""),
    ] {
        write(
            &root,
            &format!("crates/{member}/Cargo.toml"),
            &format!(
                "[package]\nname = \"x\"\n\n[dependencies]\n{dependencies}\n\n[dev-dependencies]\nunplaced = \"1\"\n"
            ),
        );
    }
    for module in [
        "auth",
        "collab",
        "collab_worker",
        "config",
        "error",
        "metrics",
        "permissions",
        "realtime",
        "scheduler",
        "search",
        "storage",
        "webhook",
    ] {
        write(&root, &format!("crates/server/src/{module}.rs"), "");
    }
    write(
        &root,
        "crates/server/src/lib.rs",
        "pub mod api; pub mod app; mod auth; mod collab; mod collab_worker; mod config; mod error; \
         mod metrics; mod permissions; mod realtime; mod scheduler; mod search; mod storage; \
         mod webhook;",
    );
    run(&root).expect_err("the planted uses fail the step");
    write(
        &root,
        "crates/server/src/api/mod.rs",
        "use crate::app::App;\nasync fn handler(app: App) {\n    app.purge().await;\n}\n",
    );
    run(&root).expect_err("one use is enough to fail the step");
    write(
        &root,
        "crates/server/src/app.rs",
        "use fvoci_store::Store;\n",
    );
    run(&root).expect("nothing breaks the rule");
    std::fs::remove_file(root.join("crates/server/src/webhook.rs")).expect("the file is removed");
    write(
        &root,
        "crates/server/src/lib.rs",
        "pub mod api; pub mod app; mod auth; mod config; mod error; mod metrics; mod permissions; \
         mod realtime; mod scheduler; mod storage;",
    );
    run(&root).expect_err("a module of the table that is gone fails the step");

    write(&root, "crates/server/src/webhook.rs", "");
    write(
        &root,
        "crates/server/src/lib.rs",
        "pub mod api; pub mod app; mod auth; mod collab; mod collab_worker; mod config; mod error; \
         mod metrics; mod permissions; mod realtime; mod scheduler; mod search; mod storage; \
         mod webhook;",
    );
    run(&root).expect("the module is back");
    write(
        &root,
        "crates/store/Cargo.toml",
        "[dependencies]\n# a comment\nsqlx.workspace = true\nbrand-new = \"1\"\n",
    );
    assert_eq!(
        undecided_dependencies(&root).expect("the manifests read"),
        [
            "crates/store/Cargo.toml: the dependency brand_new is in no layer: claim it for a layer \
          or add it to NO_LAYER, in xtask/src/layers.rs"
        ],
        "a dependency that nobody has placed"
    );
    run(&root).expect_err("and the step fails for it");
    let _ = std::fs::remove_dir_all(&root);
}

/// The worker names the documents and its frames, and of what is in no layer only what
/// its entry lists: no file, socket, database, process or runtime.
#[test]
fn the_worker_names_only_the_documents_and_what_its_entry_lists() {
    let module = "fvoci_server::collab_worker::serve";
    assert_eq!(
        broken(
            module,
            "use std::io::{self, Read};\nuse std::collections::HashMap;\n\
             use fvoci_collab::BodyDoc;\nuse fvoci_model::Body;\n\
             fn f() { let _ = io::stdin(); std::thread::sleep(std::time::Duration::ZERO); }",
        ),
        Vec::<String>::new(),
        "what it is for"
    );
    assert_eq!(
        broken(
            module,
            "use std::net::TcpStream;\nuse std::fs;\nuse tokio::net::TcpListener;\n\
             fn f() { let _ = std::process::Command::new(\"sh\"); let _ = fs::read(\"x\"); }",
        ),
        [
            "worker>no layer std::net::TcpStream",
            "worker>no layer std::fs",
            "worker>no layer tokio::net::TcpListener",
            "worker>no layer std::process::Command::new",
            "worker>no layer std::fs::read",
        ],
        "a socket, a file, a runtime and a process"
    );
    assert_eq!(
        broken(
            module,
            "use fvoci_store::Store;\nfn f(app: &App) { app.store(); }"
        ),
        ["worker>store fvoci_store::Store", "worker>store .store()"],
        "the database"
    );
    assert_eq!(
        broken(
            "fvoci_server::app::x",
            "use std::fs;\nuse fvoci_server::collab_worker::wire::Request;"
        ),
        Vec::<String>::new(),
        "the logic names files and the worker's frames"
    );
    assert_eq!(
        broken(
            "fvoci_server::collab_worker::serve::tests",
            "use std::fs;\nfn f() { let _ = fs::read(\"x\"); }"
        ),
        Vec::<String>::new(),
        "its tests read files"
    );
    assert_eq!(
        broken(
            "fvoci_collab::scan",
            "fn f() { let _ = std::net::TcpStream::connect(\"x\"); }"
        ),
        ["collab>no layer std::net::TcpStream::connect"],
        "what the worker runs of the documents opens no socket either"
    );
}
