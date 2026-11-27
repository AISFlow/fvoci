//! The list of the operations that take a token is the list of the handlers that take
//! [`Delegated`], read of the sources.

use std::collections::BTreeMap;
use std::path::Path;

use fvoci_model::ApiTokenScope;

use super::TOKEN_OPERATIONS;

/// The operations of the handlers under `api/` whose arguments take `Delegated<Read`
/// or `Delegated<Write`, by their `operationId`, and the one that takes a token alone
/// (`TokenCaller`), whose tools ask for a scope each: of either scope.
fn delegated_in_sources() -> BTreeMap<String, ApiTokenScope> {
    let api = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
    let mut found = BTreeMap::new();
    let mut pending = vec![api];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("the directory is read") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("the source is read");
            for (at, _) in source.match_indices("operation_id = \"") {
                let rest = &source[at + "operation_id = \"".len()..];
                let operation = &rest[..rest.find('"').expect("the name ends")];
                let handler = &rest[rest.find("async fn ").expect("a handler follows")..];
                let arguments = &handler[..handler.find("->").expect("the handler answers")];
                let scope =
                    if arguments.contains("Delegated<Read") || arguments.contains("TokenCaller") {
                        ApiTokenScope::Read
                    } else if arguments.contains("Delegated<Write") {
                        ApiTokenScope::Write
                    } else {
                        continue;
                    };
                found.insert(operation.to_owned(), scope);
            }
        }
    }
    found
}

#[test]
fn the_operations_that_take_a_token_are_those_whose_handlers_say_so() {
    let listed: BTreeMap<String, ApiTokenScope> = TOKEN_OPERATIONS
        .iter()
        .map(|(operation, scope)| ((*operation).to_owned(), *scope))
        .collect();
    assert_eq!(listed.len(), TOKEN_OPERATIONS.len(), "each is listed once");
    assert_eq!(listed, delegated_in_sources());
}
