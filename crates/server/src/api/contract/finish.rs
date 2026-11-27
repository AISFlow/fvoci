//! Post-processing helpers for the generated OpenAPI contract document.

use std::collections::BTreeSet;

use fvoci_model::{ApiTokenScope, PROBLEM_MEDIA_TYPE};
use serde_json::{Map, Value, json};

use super::{ContractError, METHODS, REFUSAL, object_at};

/// Takes the brackets off the names that the descriptions link to in the Rust
/// documentation they are also: ``[`Container`]`` reads `` `Container` `` in the contract.
pub(super) fn plain_descriptions(value: &mut Value) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                match member {
                    Value::String(text) if name == "description" => {
                        *text = text.replace("[`", "`").replace("`]", "`");
                    }
                    other => plain_descriptions(other),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(plain_descriptions),
        _ => {}
    }
}

/// The names in braces of a path.
pub(super) fn path_parameters(path: &str) -> impl Iterator<Item = &str> {
    path.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .map(|(name, _)| name)
}

/// Gives every operation the answer of a refusal, and checks what the attributes of the
/// handlers cannot be made to check: that each operation has a name no other has, a
/// summary and a group, that every parameter of its path is described and every path
/// parameter it describes is in its path, and that what it says a request needs is one
/// of the security schemes of the document.
pub(super) fn finish_operations(document: &mut Value) -> Result<(), ContractError> {
    let schemes: BTreeSet<String> = document
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object)
        .map(|schemes| schemes.keys().cloned().collect())
        .unwrap_or_default();
    let refusal = json!({
        "description": REFUSAL,
        "content": {
            PROBLEM_MEDIA_TYPE: { "schema": { "$ref": "#/components/schemas/Problem" } },
        },
    });
    let mut names = BTreeSet::new();
    for (path, item) in object_at(document, "/paths")? {
        for method in METHODS {
            let Some(operation) = item.get_mut(method).and_then(Value::as_object_mut) else {
                continue;
            };
            let what = format!("{method} {path}");
            let name = operation
                .get("operationId")
                .and_then(Value::as_str)
                .ok_or_else(|| ContractError(format!("{what} has no operationId")))?
                .to_owned();
            if !names.insert(name.clone()) {
                return Err(ContractError(format!(
                    "two operations have the operationId {name}"
                )));
            }
            for member in ["summary", "tags"] {
                if !operation.contains_key(member) {
                    return Err(ContractError(format!("{what} has no {member}")));
                }
            }
            for parameter in path_parameters(path) {
                let described = operation
                    .get("parameters")
                    .and_then(Value::as_array)
                    .is_some_and(|parameters| {
                        parameters.iter().any(|described| {
                            described["in"] == "path" && described["name"] == parameter
                        })
                    });
                if !described {
                    return Err(ContractError(format!(
                        "{what} does not describe its path parameter {parameter}"
                    )));
                }
            }
            let stray = operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|described| described["in"] == "path")
                .filter_map(|described| described["name"].as_str())
                .find(|name| !path_parameters(path).any(|parameter| parameter == *name));
            if let Some(stray) = stray {
                return Err(ContractError(format!(
                    "{what} describes a path parameter {stray} that its path does not have"
                )));
            }
            let unknown_scheme = operation
                .get("security")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .flat_map(serde_json::Map::keys)
                .find(|scheme| !schemes.contains(*scheme));
            if let Some(scheme) = unknown_scheme {
                return Err(ContractError(format!(
                    "{what} asks for {scheme}, which is no security scheme of the document"
                )));
            }
            let responses = operation
                .entry("responses")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| ContractError(format!("the responses of {what} are no object")))?;
            // A route a browser is sent to succeeds by sending it on (303).
            if !responses
                .keys()
                .any(|status| status.starts_with('2') || status == "303")
            {
                return Err(ContractError(format!("{what} has no answer of success")));
            }
            responses.insert("default".to_owned(), refusal.clone());
            say_whether_a_token_is_taken(operation, &name);
        }
    }
    Ok(())
}

/// Ends the description of an operation that takes a personal API token with which
/// one it takes, and marks it so (`x-fvoci-api-token`) for the clients that read it.
fn say_whether_a_token_is_taken(operation: &mut Map<String, Value>, name: &str) {
    let Some(scope) = crate::api::delegated::token_scope_of(name) else {
        return;
    };
    let said = match scope {
        ApiTokenScope::Read => {
            "A personal API token is taken, of either scope (`read` or `write`)."
        }
        ApiTokenScope::Write => {
            "A personal API token is taken that writes (`write`); one that reads only is \
             answered 403 with the reason `token_scope_insufficient`."
        }
    };
    let description = operation
        .entry("description")
        .or_insert_with(|| Value::String(String::new()));
    if let Value::String(text) = description {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(said);
    }
    operation.insert(
        "x-fvoci-api-token".to_owned(),
        Value::String(scope.to_string()),
    );
}

/// The value with the members of every object in the order of their names, whatever
/// order they were put in.
///
/// As this workspace builds `serde_json`, an object keeps its members in that order by
/// itself, and this changes nothing. It is here for the day a dependency turns on the
/// feature that keeps them in the order they were put in: the file would then change
/// from top to bottom without the API having changed at all.
pub(super) fn sorted(value: Value) -> Value {
    match value {
        Value::Object(members) => {
            let mut members: Vec<(String, Value)> = members.into_iter().collect();
            members.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                members
                    .into_iter()
                    .map(|(name, value)| (name, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}
