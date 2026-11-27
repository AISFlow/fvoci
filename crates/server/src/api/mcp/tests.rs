//! What the server tells an agent of its tools, held by a snapshot, and how it reads
//! a message.

use std::path::Path;

use fvoci_model::ApiTokenScope;
use serde_json::{Map, Value, json};

use super::rpc::{self, INVALID_PARAMS, INVALID_REQUEST, Incoming, PARSE_ERROR};
use super::tools::{TOOLS, Tool, listed_for};
use super::{PROTOCOL_VERSIONS, initialized};

/// Where the tools are as a token that writes is told of them.
const SNAPSHOT: &str = "src/api/mcp/tools.json";

/// Set to write the snapshot of what the tools are, where it differs.
const WRITE_SNAPSHOT: &str = "FVOCI_WRITE_MCP_TOOLS";

/// The names of the tools a listing lists.
fn names(listed: &Value) -> Vec<String> {
    listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name").to_owned())
        .collect()
}

#[test]
fn the_tools_are_as_the_snapshot_says() {
    let listed = listed_for(ApiTokenScope::Write);
    let mut text = serde_json::to_string_pretty(&listed).expect("JSON");
    text.push('\n');
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    if std::env::var_os(WRITE_SNAPSHOT).is_some() {
        std::fs::write(&path, &text).expect("the snapshot is written");
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        kept == text,
        "what the tools take, say or are called has changed; if that is meant, run the test \
         again with {WRITE_SNAPSHOT}=1 and look at the difference in {SNAPSHOT}"
    );
}

#[test]
fn a_token_that_reads_is_told_of_every_tool_but_those_that_write() {
    let all = names(&listed_for(ApiTokenScope::Write));
    let read = names(&listed_for(ApiTokenScope::Read));
    let writes: Vec<&str> = all
        .iter()
        .map(String::as_str)
        .filter(|name| !read.iter().any(|read| read == name))
        .collect();
    assert_eq!(writes, ["save_card", "save_page_or_unit", "post_comment"]);
    assert_eq!(all.len(), 13);
    assert_eq!(read.len(), 10);
    for tool in TOOLS {
        assert_eq!(
            read.iter().any(|name| name == tool.name()),
            tool.scope() == ApiTokenScope::Read,
            "{}",
            tool.name()
        );
    }
}

#[test]
fn each_tool_is_found_by_its_name() {
    for tool in TOOLS {
        assert_eq!(Tool::named(tool.name()), Some(tool));
    }
    assert_eq!(Tool::named("log_time"), None);
}

/// Whether `value` has a member called `member` anywhere in it.
fn has(value: &Value, member: &str) -> bool {
    match value {
        Value::Object(members) => members
            .iter()
            .any(|(name, value)| name == member || has(value, member)),
        Value::Array(values) => values.iter().any(|value| has(value, member)),
        _ => false,
    }
}

#[test]
fn each_input_schema_is_an_object_that_takes_nothing_else_and_refers_nowhere() {
    for tool in TOOLS {
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object", "{}", tool.name());
        assert_eq!(
            schema["additionalProperties"],
            false,
            "{} takes no other argument",
            tool.name()
        );
        assert!(!has(&schema, "$ref"), "{} is whole in itself", tool.name());
        // What a schema generator joins of a type and what is said of its field: an
        // enum joined so with an object is a value nothing is.
        assert!(
            !has(&schema, "allOf"),
            "{} joins no two schemas",
            tool.name()
        );
    }
}

#[test]
fn the_body_of_a_request_is_one_message() {
    assert_eq!(
        rpc::read(b"{").map_err(|error| error.code),
        Err(PARSE_ERROR)
    );
    let batch = br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
    assert_eq!(
        rpc::read(batch).map_err(|error| error.code),
        Err(INVALID_REQUEST)
    );
    for refused in [
        r#"{"id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":7}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
    ] {
        assert_eq!(
            rpc::read(refused.as_bytes()).map_err(|error| error.code),
            Err(INVALID_REQUEST),
            "{refused}"
        );
    }
}

#[test]
fn a_batch_is_refused_before_it_is_read_and_a_request_with_wrong_params_by_its_id() {
    // No JSON, and refused all the same as a batch.
    assert_eq!(
        rpc::read(b"  [ not even JSON").map_err(|error| error.code),
        Err(INVALID_REQUEST)
    );
    let wrong = br#"{"jsonrpc":"2.0","id":4,"method":"ping","params":[1]}"#;
    match rpc::read(wrong) {
        Ok(Incoming::Refused { id, error }) => {
            assert_eq!(id, json!(4));
            assert_eq!(error.code, INVALID_PARAMS);
        }
        other => panic!("a request with params that are no object: {other:?}"),
    }
    let notified = br#"{"jsonrpc":"2.0","method":"ping","params":[1]}"#;
    assert_eq!(
        rpc::read(notified).map_err(|error| error.code),
        Err(INVALID_REQUEST)
    );
}

#[test]
fn a_request_is_read_with_its_id_and_a_notification_is_taken() {
    let request = br#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#;
    assert_eq!(
        rpc::read(request),
        Ok(Incoming::Request {
            id: json!("a"),
            method: "tools/list".to_owned(),
            params: Map::new(),
        })
    );
    for taken in [
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":3,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"error":{"code":1,"message":"no"}}"#,
    ] {
        assert_eq!(
            rpc::read(taken.as_bytes()),
            Ok(Incoming::Accepted),
            "{taken}"
        );
    }
}

#[test]
fn initialize_answers_with_the_version_asked_for_if_it_is_spoken() {
    for version in PROTOCOL_VERSIONS {
        let params = json!({ "protocolVersion": version });
        let answer = initialized(params.as_object().expect("an object"));
        assert_eq!(answer["protocolVersion"], version);
        assert_eq!(
            answer["capabilities"],
            json!({ "tools": { "listChanged": false } })
        );
    }
    let params = json!({ "protocolVersion": "2024-11-05" });
    let answer = initialized(params.as_object().expect("an object"));
    assert_eq!(answer["protocolVersion"], "2025-11-25");
}
