//! The 21 source tools (`apps/mcp/src/server.ts`): each maps validated
//! arguments to one FVOCI API request. Names, descriptions and input schemas
//! come verbatim from `tools.json`, generated from the source server.

use serde_json::{json, Map, Value};

/// One API request a tool call resolves to.
#[derive(Debug, PartialEq)]
pub struct ApiCall {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
    pub query: Vec<(&'static str, String)>,
    /// `list_labels` / `list_task_dependencies` return `items` only.
    pub unwrap_items: bool,
}

/// Fields the source schema trims before validating (`z.string().trim()`).
pub fn trimmed_fields(tool: &str) -> &'static [&'static str] {
    match tool {
        "create_task" | "patch_task" => &["title"],
        "add_comment" => &["body"],
        "create_label" | "patch_label" => &["name"],
        _ => &[],
    }
}

fn s<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn req<'a>(args: &'a Map<String, Value>, key: &str) -> &'a str {
    // Required ids are enforced by the schema before `route` runs.
    s(args, key).unwrap_or_default()
}

fn ws(args: &Map<String, Value>) -> String {
    format!("/api/v1/workspaces/{}", req(args, "workspaceId"))
}

fn project(args: &Map<String, Value>) -> String {
    format!("{}/projects/{}", ws(args), req(args, "projectId"))
}

/// Wiki pair: the project-scoped path when `projectId` is given.
fn documents(args: &Map<String, Value>) -> String {
    match s(args, "projectId") {
        Some(p) => format!("{}/projects/{p}/documents", ws(args)),
        None => format!("{}/documents", ws(args)),
    }
}

/// `compactQuery`: present values only, numbers and booleans as strings, in
/// the source key order.
fn query(args: &Map<String, Value>, keys: &[&'static str]) -> Vec<(&'static str, String)> {
    keys.iter()
        .filter_map(|&k| {
            let v = args.get(k)?;
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                // `query` (view query) travels as its JSON text.
                other => other.to_string(),
            };
            Some((k, text))
        })
        .collect()
}

/// The request body: the given keys that are present (JSON.stringify drops
/// `undefined`; explicit `null` is kept).
fn pick(args: &Map<String, Value>, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for &k in keys {
        if let Some(v) = args.get(k) {
            out.insert(k.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

fn call(method: &'static str, path: String) -> ApiCall {
    ApiCall {
        method,
        path,
        body: None,
        query: Vec::new(),
        unwrap_items: false,
    }
}

/// Maps validated arguments to the API request, or a tool error message.
pub fn route(tool: &str, args: &Map<String, Value>) -> Result<ApiCall, String> {
    let c = match tool {
        "list_tasks" => ApiCall {
            query: query(
                args,
                &["query", "archived", "cursor", "limit", "from", "to"],
            ),
            ..call("GET", format!("{}/tasks", project(args)))
        },
        "get_task" => call("GET", format!("{}/tasks/{}", ws(args), req(args, "id"))),
        "list_task_activity" => ApiCall {
            query: query(args, &["filter", "cursor", "limit"]),
            ..call(
                "GET",
                format!("{}/tasks/{}/activity", ws(args), req(args, "id")),
            )
        },
        "create_task" => ApiCall {
            body: Some(pick(
                args,
                &[
                    "type",
                    "title",
                    "priority",
                    "statusId",
                    "startDate",
                    "dueDate",
                    "parentId",
                    "milestoneId",
                ],
            )),
            ..call("POST", format!("{}/tasks", project(args)))
        },
        "patch_task" => ApiCall {
            body: Some(pick(
                args,
                &[
                    "type",
                    "title",
                    "priority",
                    "statusId",
                    "startDate",
                    "dueDate",
                    "dueAt",
                    "estimate",
                    "parentId",
                    "milestoneId",
                    "archived",
                    "assigneeIds",
                    "labelIds",
                ],
            )),
            ..call("PATCH", format!("{}/tasks/{}", ws(args), req(args, "id")))
        },
        "add_comment" => {
            let body = pick(
                args,
                &["body", "parentId", "mentionedUserIds", "mentionedGroupIds"],
            );
            match (s(args, "documentId"), s(args, "taskId")) {
                (Some(doc), None) => ApiCall {
                    body: Some(body),
                    ..call("POST", format!("{}/{doc}/comments", documents(args)))
                },
                (None, Some(task)) => ApiCall {
                    body: Some(body),
                    ..call("POST", format!("{}/tasks/{task}/comments", ws(args)))
                },
                _ => return Err("exactly one of documentId or taskId is required".into()),
            }
        }
        "resolve_comment" => call(
            "POST",
            format!("{}/comments/{}/resolve", ws(args), req(args, "id")),
        ),
        "list_labels" => ApiCall {
            unwrap_items: true,
            ..call("GET", format!("{}/labels", project(args)))
        },
        "create_label" => ApiCall {
            body: Some(pick(args, &["name", "color"])),
            ..call("POST", format!("{}/labels", project(args)))
        },
        "patch_label" => ApiCall {
            body: Some(pick(args, &["name", "color"])),
            ..call(
                "PATCH",
                format!("{}/labels/{}", project(args), req(args, "id")),
            )
        },
        "list_task_dependencies" => ApiCall {
            unwrap_items: true,
            ..call("GET", format!("{}/dependencies", project(args)))
        },
        "add_task_dependency" => ApiCall {
            body: Some(pick(args, &["blockedId", "type", "lagDays"])),
            ..call(
                "POST",
                format!("{}/tasks/{}/dependencies", ws(args), req(args, "id")),
            )
        },
        "remove_task_dependency" => call(
            "DELETE",
            format!(
                "{}/tasks/{}/dependencies/{}",
                ws(args),
                req(args, "id"),
                req(args, "blockedId")
            ),
        ),
        "search" => ApiCall {
            query: query(args, &["q", "type", "cursor", "limit"]),
            ..call("GET", format!("{}/search", ws(args)))
        },
        "get_document_body" => ApiCall {
            query: query(args, &["format"]),
            ..call(
                "GET",
                format!("{}/{}/body", documents(args), req(args, "id")),
            )
        },
        "put_document_body" => {
            let body = match (args.get("contentMd"), args.get("contentJson")) {
                (Some(md), None) => json!({ "contentMd": md }),
                (None, Some(doc)) => json!({ "contentJson": doc }),
                _ => return Err("exactly one of contentMd or contentJson is required".into()),
            };
            ApiCall {
                body: Some(body),
                ..call(
                    "PUT",
                    format!("{}/{}/body", documents(args), req(args, "id")),
                )
            }
        }
        "patch_document_block" => ApiCall {
            body: Some(pick(args, &["type", "attrs", "content", "marks", "text"])),
            ..call(
                "PATCH",
                format!(
                    "{}/{}/blocks/{}",
                    documents(args),
                    req(args, "id"),
                    req(args, "blockId")
                ),
            )
        },
        "get_calendar" => ApiCall {
            query: query(args, &["from", "to", "query", "archived", "limit"]),
            ..call("GET", format!("{}/tasks", project(args)))
        },
        "ai_summarize_document" | "ai_generate_tasks" | "ai_suggest_links" => {
            let action = match tool {
                "ai_summarize_document" => "summarize",
                "ai_generate_tasks" => "generate-tasks",
                _ => "suggest-links",
            };
            ApiCall {
                body: Some(pick(args, &["documentId"])),
                ..call("POST", format!("{}/ai/{action}", ws(args)))
            }
        }
        other => return Err(format!("MCP error -32602: Tool {other} not found")),
    };
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "11111111-1111-4111-8111-111111111111";
    const P: &str = "22222222-2222-4222-8222-222222222222";
    const D: &str = "44444444-4444-4444-8444-444444444444";

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn wiki_pair_follows_project_id() {
        let wiki = route(
            "get_document_body",
            &args(json!({"workspaceId": WS, "id": D, "format": "md"})),
        )
        .unwrap();
        assert_eq!(
            wiki.path,
            format!("/api/v1/workspaces/{WS}/documents/{D}/body")
        );
        assert_eq!(wiki.query, vec![("format", "md".to_string())]);
        let proj = route(
            "get_document_body",
            &args(json!({"workspaceId": WS, "projectId": P, "id": D})),
        )
        .unwrap();
        assert_eq!(
            proj.path,
            format!("/api/v1/workspaces/{WS}/projects/{P}/documents/{D}/body")
        );
    }

    #[test]
    fn comment_needs_exactly_one_parent() {
        let err = route(
            "add_comment",
            &args(json!({"workspaceId": WS, "body": "x", "documentId": D, "taskId": D})),
        )
        .unwrap_err();
        assert_eq!(err, "exactly one of documentId or taskId is required");
    }

    #[test]
    fn list_tasks_query_order_and_scalars() {
        let c = route(
            "list_tasks",
            &args(
                json!({"workspaceId": WS, "projectId": P, "limit": 5, "archived": false,
                "query": {"filters": {}, "sort": []}}),
            ),
        )
        .unwrap();
        assert_eq!(
            c.query,
            vec![
                ("query", r#"{"filters":{},"sort":[]}"#.to_string()),
                ("archived", "false".to_string()),
                ("limit", "5".to_string()),
            ]
        );
    }
}
