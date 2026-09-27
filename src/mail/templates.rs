//! Korean copy matches packages/i18n/src/locales/ko.json at the source SHA.
//! Strings an operator may override (`i18n.overrides`) live in
//! `crate::settings::messages`; these are not overridable.

pub const COMMENT_SUBJECT: &str = "새 댓글이 있습니다";
pub const DIGEST_SUBJECT: &str = "알림 다이제스트";
pub const WITHDRAW_CANCEL_SUBJECT: &str = "FVOCI 탈퇴 취소";

pub const MAGIC_TTL_MINUTES: i64 = 15;

pub fn withdraw_cancel_text(url: &str) -> String {
    format!(
        "계정 삭제가 예약되었습니다. 14일 뒤 이름·이메일과 개인 워크스페이스가 정리됩니다. 삭제를 원하지 않으면 기한 전에 아래 링크에서 취소하거나 관리자에게 요청하세요. 링크를 여는 것만으로는 취소되지 않습니다.\n{url}"
    )
}

pub fn comment_text(actor: &str, body: &str) -> String {
    format!("{actor}님이 댓글을 남겼습니다.\n\n{body}")
}

pub fn digest_text(count: i64) -> String {
    format!("읽지 않은 알림이 {count}건 있습니다.")
}

/// `mail.magic.link.text` (login, reset and email-change links).
pub fn magic_link_text(messages: &crate::settings::messages::Messages, url: &str) -> String {
    messages.render(
        crate::settings::messages::Message::MagicLinkText,
        &[("url", url), ("minutes", &MAGIC_TTL_MINUTES.to_string())],
    )
}
