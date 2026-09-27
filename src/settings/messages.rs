//! Server-sent strings with the operator's `i18n.overrides` applied (source
//! `t()` after `setInstanceOverrides`): the stored override when the key has
//! one, otherwise the Korean catalog default. Callers load the map when the
//! string is used (send, seed, clone, anonymize), so a PATCH applies to the
//! next use in every process without a restart.

use std::collections::BTreeMap;

use serde_json::Value;

use super::catalog::{SettingsKey, SettingsValues};

/// The overridable keys of `catalog::OVERRIDABLE_MESSAGES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    SeedStatusBacklog,
    SeedStatusTodo,
    SeedStatusInProgress,
    SeedStatusReview,
    SeedStatusDone,
    SeedStatusCanceled,
    MagicLoginSubject,
    MagicResetSubject,
    EmailChangeSubject,
    EmailChangeRequestedSubject,
    EmailChangeCompletedSubject,
    MagicLinkText,
    EmailChangeRequestedText,
    EmailChangeCompletedText,
    InviteSubject,
    IdentityLinkedSubject,
    IdentityLinkedText,
    IdentityUnlinkedSubject,
    IdentityUnlinkedText,
    WithdrawnDisplayName,
    TaskDuplicateSuffix,
}

impl Message {
    pub const ALL: [Message; 21] = [
        Message::SeedStatusBacklog,
        Message::SeedStatusTodo,
        Message::SeedStatusInProgress,
        Message::SeedStatusReview,
        Message::SeedStatusDone,
        Message::SeedStatusCanceled,
        Message::MagicLoginSubject,
        Message::MagicResetSubject,
        Message::EmailChangeSubject,
        Message::EmailChangeRequestedSubject,
        Message::EmailChangeCompletedSubject,
        Message::MagicLinkText,
        Message::EmailChangeRequestedText,
        Message::EmailChangeCompletedText,
        Message::InviteSubject,
        Message::IdentityLinkedSubject,
        Message::IdentityLinkedText,
        Message::IdentityUnlinkedSubject,
        Message::IdentityUnlinkedText,
        Message::WithdrawnDisplayName,
        Message::TaskDuplicateSuffix,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::SeedStatusBacklog => "seed.status.backlog",
            Self::SeedStatusTodo => "seed.status.todo",
            Self::SeedStatusInProgress => "seed.status.in_progress",
            Self::SeedStatusReview => "seed.status.review",
            Self::SeedStatusDone => "seed.status.done",
            Self::SeedStatusCanceled => "seed.status.canceled",
            Self::MagicLoginSubject => "mail.magic.login.subject",
            Self::MagicResetSubject => "mail.magic.reset.subject",
            Self::EmailChangeSubject => "mail.magic.emailChange.subject",
            Self::EmailChangeRequestedSubject => "mail.magic.emailChangeRequested.subject",
            Self::EmailChangeCompletedSubject => "mail.magic.emailChangeCompleted.subject",
            Self::MagicLinkText => "mail.magic.link.text",
            Self::EmailChangeRequestedText => "mail.magic.emailChangeRequested.text",
            Self::EmailChangeCompletedText => "mail.magic.emailChangeCompleted.text",
            Self::InviteSubject => "mail.invite.subject",
            Self::IdentityLinkedSubject => "mail.identity.linked.subject",
            Self::IdentityLinkedText => "mail.identity.linked.text",
            Self::IdentityUnlinkedSubject => "mail.identity.unlinked.subject",
            Self::IdentityUnlinkedText => "mail.identity.unlinked.text",
            Self::WithdrawnDisplayName => "withdrawn.displayName",
            Self::TaskDuplicateSuffix => "task.duplicate.suffix",
        }
    }

    /// Korean copy of packages/i18n/src/locales/ko.json at the source SHA.
    pub fn default_text(self) -> &'static str {
        match self {
            Self::SeedStatusBacklog => "백로그",
            Self::SeedStatusTodo => "할 일",
            Self::SeedStatusInProgress => "진행 중",
            Self::SeedStatusReview => "검토 대기",
            Self::SeedStatusDone => "완료",
            Self::SeedStatusCanceled => "취소",
            Self::MagicLoginSubject => "FVOCI 로그인 링크",
            Self::MagicResetSubject => "FVOCI 비밀번호 재설정",
            Self::EmailChangeSubject => "FVOCI 이메일 변경 확인",
            Self::EmailChangeRequestedSubject => "FVOCI 이메일 변경 요청 알림",
            Self::EmailChangeCompletedSubject => "FVOCI 이메일 변경 완료 알림",
            Self::MagicLinkText => {
                "{{url}}\n{{minutes}}분 안에 사용할 수 있습니다. 본인이 요청하지 않았다면 무시하세요."
            }
            Self::EmailChangeRequestedText => {
                "이 계정의 이메일 변경이 요청되었습니다. 본인이 아니면 이 메일을 무시하고 비밀번호를 변경하세요."
            }
            Self::EmailChangeCompletedText => {
                "이 계정의 이메일이 변경되었습니다. 본인이 아니면 관리자에게 문의하세요."
            }
            Self::InviteSubject => "워크스페이스 초대",
            Self::IdentityLinkedSubject => "[FVOCI] 소셜 로그인 연결 알림",
            Self::IdentityLinkedText => {
                "이 계정에 소셜 로그인({{provider}})이 연결되었습니다.\n\n본인이 연결한 것이 아니라면 계정 설정에서 연결을 해제하고 비밀번호를 변경하세요."
            }
            Self::IdentityUnlinkedSubject => "[FVOCI] 소셜 로그인 연결 해제 알림",
            Self::IdentityUnlinkedText => {
                "이 계정에서 소셜 로그인({{provider}}) 연결이 해제되었습니다.\n\n본인이 해제한 것이 아니라면 계정 설정을 확인하고 비밀번호를 변경하세요."
            }
            Self::WithdrawnDisplayName => "탈퇴한 사용자",
            Self::TaskDuplicateSuffix => "{{title}} 복사",
        }
    }
}

/// One read of the override map. Only whitelisted keys that passed the
/// catalog schema are present, so every template keeps its exact `{{vars}}`.
#[derive(Debug, Clone, Default)]
pub struct Messages {
    overrides: BTreeMap<String, String>,
}

impl Messages {
    pub fn defaults() -> Self {
        Self::default()
    }

    /// From a stored `i18n` row; an invalid row falls back to the defaults
    /// like every other settings key.
    pub fn from_row(raw: Option<&Value>) -> Self {
        let mut values = SettingsValues::defaults("FVOCI");
        if let Some(raw) = raw {
            if values.set_json(SettingsKey::I18n, raw).is_none() {
                tracing::warn!(key = "i18n", reason = "schema", "settings.row_invalid");
            }
        }
        Self {
            overrides: values.i18n.overrides,
        }
    }

    /// The template: override, else the Korean default.
    pub fn template(&self, message: Message) -> &str {
        self.overrides
            .get(message.key())
            .map(String::as_str)
            .unwrap_or_else(|| message.default_text())
    }

    /// `template` with `{{name}}` replaced in one pass (source `t()`):
    /// substituted values are never rescanned, so a title or URL containing
    /// `{{…}}` stays literal. Names without a value are left as written.
    pub fn render(&self, message: Message, vars: &[(&str, &str)]) -> String {
        interpolate(self.template(message), vars)
    }

    /// A header-safe subject: control characters (an override may contain
    /// line breaks) become spaces so the text cannot start another header.
    pub fn subject(&self, message: Message) -> String {
        self.template(message)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }

    /// `template` for a stored field with its own limit: a valid override
    /// that the field would reject falls back to the Korean default.
    pub fn field(&self, message: Message, fits: impl Fn(&str) -> bool) -> &str {
        let text = self.template(message);
        if fits(text) {
            text
        } else {
            message.default_text()
        }
    }
}

fn interpolate(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        let value = (!name.is_empty() && after[name_len..].starts_with("}}"))
            .then(|| vars.iter().find(|(key, _)| *key == name))
            .flatten();
        match value {
            Some((_, value)) => {
                out.push_str(value);
                rest = &after[name_len + 2..];
            }
            None => {
                // Not a placeholder: keep one brace and rescan from the next
                // (`{{{url}}}` renders `{u}` like the source regex).
                out.push('{');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Loads the override map inside the caller's transaction or pool.
/// `instance_settings` is readable in every context (SELECT policy `true`).
pub async fn load<'e, E>(executor: E) -> Result<Messages, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let raw: Option<Value> =
        sqlx::query_scalar("SELECT value FROM fvoci.instance_settings WHERE key = 'i18n'")
            .fetch_optional(executor)
            .await?;
    Ok(Messages::from_row(raw.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_catalog_key_has_a_message_with_its_default_variables() {
        let keys: Vec<&str> = Message::ALL.iter().map(|m| m.key()).collect();
        let catalog: Vec<&str> = super::super::catalog::OVERRIDABLE_MESSAGES
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(keys, catalog);
        // Each default must itself pass the override schema (exact vars).
        let overrides: serde_json::Map<String, Value> = Message::ALL
            .iter()
            .map(|m| (m.key().to_string(), json!(m.default_text())))
            .collect();
        let doc = json!({ "overrides": overrides });
        assert!(
            super::super::catalog::parse_patch_value(SettingsKey::I18n, &doc).is_some(),
            "defaults keep the catalog variables"
        );
    }

    #[test]
    fn overrides_apply_and_missing_keys_use_the_default() {
        let messages = Messages::from_row(Some(&json!({"overrides": {
            "mail.invite.subject": "Join us",
            "task.duplicate.suffix": "Copy of {{title}}"
        }})));
        assert_eq!(messages.template(Message::InviteSubject), "Join us");
        assert_eq!(
            messages.render(Message::TaskDuplicateSuffix, &[("title", "A")]),
            "Copy of A"
        );
        assert_eq!(
            messages.template(Message::MagicLoginSubject),
            "FVOCI 로그인 링크"
        );
        assert_eq!(
            Messages::defaults().render(Message::TaskDuplicateSuffix, &[("title", "A")]),
            "A 복사"
        );
    }

    #[test]
    fn invalid_row_falls_back_to_defaults() {
        let broken = json!({"overrides": {"mail.magic.link.text": "no vars"}});
        let messages = Messages::from_row(Some(&broken));
        assert_eq!(
            messages.template(Message::MagicLinkText),
            Message::MagicLinkText.default_text()
        );
        let unknown = json!({"overrides": {"mail.comment.subject": "x"}});
        assert_eq!(
            Messages::from_row(Some(&unknown)).template(Message::InviteSubject),
            "워크스페이스 초대"
        );
    }

    #[test]
    fn interpolation_is_single_pass() {
        assert_eq!(
            interpolate("{{title}} 복사", &[("title", "{{title}} x")]),
            "{{title}} x 복사"
        );
        assert_eq!(
            interpolate(
                "{{url}}\n{{minutes}}",
                &[("url", "{{minutes}}"), ("minutes", "15")]
            ),
            "{{minutes}}\n15"
        );
        assert_eq!(
            interpolate("{{other}} {{ url}} {{", &[("url", "u")]),
            "{{other}} {{ url}} {{"
        );
        assert_eq!(interpolate("{{{url}}}", &[("url", "u")]), "{u}");
        assert_eq!(
            interpolate("한글 {{url}}끝", &[("url", "링크")]),
            "한글 링크끝"
        );
    }

    #[test]
    fn subjects_cannot_carry_line_breaks() {
        let messages = Messages::from_row(Some(&json!({"overrides": {
            "mail.invite.subject": "Hi\r\nBcc: evil@example.com"
        }})));
        assert_eq!(
            messages.subject(Message::InviteSubject),
            "Hi  Bcc: evil@example.com"
        );
    }

    #[test]
    fn field_limits_fall_back_to_the_default() {
        let long = "가".repeat(101);
        let messages = Messages::from_row(Some(&json!({"overrides": {
            "seed.status.todo": long,
            "seed.status.done": "Shipped"
        }})));
        let fits = |s: &str| s.encode_utf16().count() <= 100;
        assert_eq!(messages.field(Message::SeedStatusTodo, fits), "할 일");
        assert_eq!(messages.field(Message::SeedStatusDone, fits), "Shipped");
    }
}
