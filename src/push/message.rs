//! Push text for one notification row: source `notificationMessage`
//! (`packages/i18n`, default `ko` locale) and `humanPaths`. The inbox and the
//! browser push must read the same sentence. The server image does not ship
//! `packages/`, so the catalog strings are copied here and a test pins them to
//! `packages/i18n/src/locales/ko.json`.

use serde_json::Value;

const CATALOG: &[(&str, &str)] = &[
    (
        "notif.task.created.ref",
        "새 태스크 {{ref}}의 담당자로 지정되었습니다",
    ),
    ("notif.task.created", "새 태스크의 담당자로 지정되었습니다"),
    (
        "notif.task.status",
        "태스크 {{ref}} 상태: {{from}} → {{to}}",
    ),
    ("notif.task.statusPlain", "태스크 상태: {{from}} → {{to}}"),
    (
        "notif.task.assignee.ref",
        "태스크 {{ref}}의 담당자로 지정되었습니다",
    ),
    ("notif.task.assignee", "태스크의 담당자로 지정되었습니다"),
    (
        "notif.task.deleted.number",
        "담당 태스크(#{{number}})가 삭제되었습니다",
    ),
    ("notif.task.deleted", "담당 태스크가 삭제되었습니다"),
    ("notif.project.named", "「{{name}}」 프로젝트"),
    ("nav.projects", "프로젝트"),
    (
        "notif.project.added.role",
        "{{target}}에 {{role}}{{particle}} 추가되었습니다",
    ),
    ("notif.project.added", "{{target}}에 추가되었습니다"),
    (
        "notif.project.role.changed.role",
        "{{target}} 역할이 {{role}}{{particle}} 변경되었습니다",
    ),
    (
        "notif.project.role.changed",
        "{{target}} 역할이 변경되었습니다",
    ),
    ("projectRole.lead", "책임자"),
    ("projectRole.member", "멤버"),
    ("projectRole.viewer", "뷰어"),
    ("particle.ro", "로"),
    ("particle.euro", "으로"),
    ("particle.euroParen", "(으)로"),
    ("notif.invite.accepted", "보낸 초대가 수락되었습니다"),
    ("notif.comment.reply", "댓글에 답글이 달렸습니다"),
    ("notif.comment.task", "태스크에 새 댓글이 달렸습니다"),
    ("notif.comment.document", "문서에 새 댓글이 달렸습니다"),
    ("notif.comment.created", "새 댓글이 달렸습니다"),
    (
        "notif.comment.resolved.task",
        "태스크의 댓글 스레드가 해결되었습니다",
    ),
    (
        "notif.comment.resolved.document",
        "문서의 댓글 스레드가 해결되었습니다",
    ),
    ("notif.comment.resolved", "댓글 스레드가 해결되었습니다"),
    ("notif.generic", "새 알림이 있습니다"),
];

fn t(key: &str, params: &[(&str, &str)]) -> String {
    let template = CATALOG
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or(key);
    params
        .iter()
        .fold(template.to_string(), |out, (name, value)| {
            out.replace(&format!("{{{{{name}}}}}"), value)
        })
}

/// Payload strings are jsonb: a non-string or empty value counts as absent.
fn str_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn task_ref(payload: &Value) -> Option<String> {
    let title = str_field(payload, "title");
    let number = payload.get("number").and_then(Value::as_i64);
    match (title, number) {
        (Some(title), Some(number)) => Some(format!("#{number} 「{title}」")),
        (Some(title), None) => Some(format!("「{title}」")),
        (None, Some(number)) => Some(format!("#{number}")),
        (None, None) => None,
    }
}

/// I18N-6: 로/으로 by the final syllable's batchim; non-Hangul keeps both.
fn ro_particle(word: &str) -> String {
    let code = word.chars().last().map(u32::from).unwrap_or(0);
    if !(0xac00..=0xd7a3).contains(&code) {
        return t("particle.euroParen", &[]);
    }
    if (code - 0xac00).is_multiple_of(28) {
        t("particle.ro", &[])
    } else {
        t("particle.euro", &[])
    }
}

fn role_label(payload: &Value) -> Option<String> {
    let key = match str_field(payload, "role")? {
        "lead" => "projectRole.lead",
        "member" => "projectRole.member",
        "viewer" => "projectRole.viewer",
        _ => return None,
    };
    Some(t(key, &[]))
}

pub fn notification_body(verb: &str, payload: &Value) -> String {
    let reference = task_ref(payload);
    let with_ref = |key_ref: &str, key_plain: &str| match &reference {
        Some(r) => t(key_ref, &[("ref", r)]),
        None => t(key_plain, &[]),
    };
    match verb {
        "task.created" => with_ref("notif.task.created.ref", "notif.task.created"),
        "task.updated" => match (str_field(payload, "fromName"), str_field(payload, "toName")) {
            (Some(from), Some(to)) => match &reference {
                Some(r) => t(
                    "notif.task.status",
                    &[("ref", r), ("from", from), ("to", to)],
                ),
                None => t("notif.task.statusPlain", &[("from", from), ("to", to)]),
            },
            _ => with_ref("notif.task.assignee.ref", "notif.task.assignee"),
        },
        "task.deleted" => match payload.get("number").and_then(Value::as_i64) {
            Some(number) => t(
                "notif.task.deleted.number",
                &[("number", &number.to_string())],
            ),
            None => t("notif.task.deleted", &[]),
        },
        "project_member.added" | "project_member.role_changed" => {
            let target = match str_field(payload, "projectName") {
                Some(name) => t("notif.project.named", &[("name", name)]),
                None => t("nav.projects", &[]),
            };
            let added = verb == "project_member.added";
            match role_label(payload) {
                None if added => t("notif.project.added", &[("target", &target)]),
                None => t("notif.project.role.changed", &[("target", &target)]),
                Some(role) => {
                    let particle = ro_particle(&role);
                    let params = [
                        ("target", target.as_str()),
                        ("role", role.as_str()),
                        ("particle", particle.as_str()),
                    ];
                    if added {
                        t("notif.project.added.role", &params)
                    } else {
                        t("notif.project.role.changed.role", &params)
                    }
                }
            }
        }
        "invitation.accepted" => t("notif.invite.accepted", &[]),
        "comment.created" => {
            if str_field(payload, "parentId").is_some() {
                t("notif.comment.reply", &[])
            } else if str_field(payload, "taskId").is_some() {
                t("notif.comment.task", &[])
            } else if str_field(payload, "documentId").is_some() {
                t("notif.comment.document", &[])
            } else {
                t("notif.comment.created", &[])
            }
        }
        "comment.resolved" => {
            if str_field(payload, "taskId").is_some() {
                t("notif.comment.resolved.task", &[])
            } else if str_field(payload, "documentId").is_some() {
                t("notif.comment.resolved.document", &[])
            } else {
                t("notif.comment.resolved", &[])
            }
        }
        _ => t("notif.generic", &[]),
    }
}

/// Source `humanPaths.item` / `humanPaths.notifications`.
pub fn push_url(slug: &str, display_id: Option<&str>) -> String {
    match display_id {
        Some(display_id) => format!("/w/{slug}/{display_id}"),
        None => format!("/w/{slug}/notifications"),
    }
}

/// Source `formatPersonName(actor, actor.locale)`.
pub fn format_person_name(given_name: &str, family_name: Option<&str>, locale: &str) -> String {
    let given = given_name.trim();
    match family_name.map(str::trim).filter(|value| !value.is_empty()) {
        None => given.to_string(),
        Some(family) if locale == "ko" => format!("{family}{given}"),
        Some(family) => format!("{given} {family}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_matches_web_i18n_ko() {
        let ko: serde_json::Map<String, Value> =
            serde_json::from_str(include_str!("../../packages/i18n/src/locales/ko.json")).unwrap();
        for (key, text) in CATALOG {
            assert_eq!(ko.get(*key).and_then(Value::as_str), Some(*text), "{key}");
        }
    }

    #[test]
    fn messages_follow_source_rules() {
        assert_eq!(
            notification_body(
                "task.updated",
                &json!({ "number": 3, "title": "알림 수신 확인 태스크" })
            ),
            "태스크 #3 「알림 수신 확인 태스크」의 담당자로 지정되었습니다"
        );
        assert_eq!(
            notification_body("task.updated", &json!({ "from": "a", "to": "b" })),
            "태스크의 담당자로 지정되었습니다"
        );
        assert_eq!(
            notification_body(
                "task.updated",
                &json!({ "number": 1, "fromName": "할 일", "toName": "완료" })
            ),
            "태스크 #1 상태: 할 일 → 완료"
        );
        assert_eq!(
            notification_body(
                "project_member.added",
                &json!({ "projectName": "알파", "role": "lead" })
            ),
            "「알파」 프로젝트에 책임자로 추가되었습니다"
        );
        assert_eq!(
            notification_body("project_member.role_changed", &json!({ "role": "member" })),
            "프로젝트 역할이 멤버로 변경되었습니다"
        );
        assert_eq!(
            notification_body("project_member.added", &json!({ "role": "viewer" })),
            "프로젝트에 뷰어로 추가되었습니다"
        );
        assert_eq!(
            notification_body("comment.created", &json!({ "taskId": "x", "parentId": "" })),
            "태스크에 새 댓글이 달렸습니다"
        );
        assert_eq!(
            notification_body("task.deleted", &json!({ "number": 9 })),
            "담당 태스크(#9)가 삭제되었습니다"
        );
        assert_eq!(
            notification_body("webhook.created", &json!({})),
            "새 알림이 있습니다"
        );
        assert_eq!(ro_particle("책임자"), "로");
        assert_eq!(ro_particle("멤버"), "로");
        assert_eq!(ro_particle("팀원"), "으로");
        assert_eq!(ro_particle("QA"), "(으)로");
    }

    #[test]
    fn urls_and_names() {
        assert_eq!(push_url("acme", Some("ACME-4")), "/w/acme/ACME-4");
        assert_eq!(push_url("acme", None), "/w/acme/notifications");
        assert_eq!(format_person_name("길동", Some("홍"), "ko"), "홍길동");
        assert_eq!(
            format_person_name("Ada", Some("Lovelace"), "en"),
            "Ada Lovelace"
        );
        assert_eq!(format_person_name(" Ada ", Some("  "), "en"), "Ada");
    }
}
