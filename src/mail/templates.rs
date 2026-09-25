//! Korean copy matches packages/i18n/src/locales/ko.json at the source SHA.

pub const INVITE_SUBJECT: &str = "워크스페이스 초대";
pub const RESET_SUBJECT: &str = "FVOCI 비밀번호 재설정";
pub const COMMENT_SUBJECT: &str = "새 댓글이 있습니다";
pub const DIGEST_SUBJECT: &str = "알림 다이제스트";
pub const IDENTITY_LINKED_SUBJECT: &str = "[FVOCI] 소셜 로그인 연결 알림";
pub const IDENTITY_UNLINKED_SUBJECT: &str = "[FVOCI] 소셜 로그인 연결 해제 알림";

pub const MAGIC_LOGIN_SUBJECT: &str = "FVOCI 로그인 링크";
pub const EMAIL_CHANGE_SUBJECT: &str = "FVOCI 이메일 변경 확인";
pub const EMAIL_CHANGE_REQUESTED_SUBJECT: &str = "FVOCI 이메일 변경 요청 알림";
pub const EMAIL_CHANGE_REQUESTED_TEXT: &str =
    "이 계정의 이메일 변경이 요청되었습니다. 본인이 아니면 이 메일을 무시하고 비밀번호를 변경하세요.";
pub const EMAIL_CHANGE_COMPLETED_SUBJECT: &str = "FVOCI 이메일 변경 완료 알림";
pub const EMAIL_CHANGE_COMPLETED_TEXT: &str =
    "이 계정의 이메일이 변경되었습니다. 본인이 아니면 관리자에게 문의하세요.";
pub const WITHDRAW_CANCEL_SUBJECT: &str = "FVOCI 탈퇴 취소";

pub const MAGIC_TTL_MINUTES: i64 = 15;

/// Source `mail.magic.link.text` (login, reset and email-change links).
pub fn magic_link_text(url: &str) -> String {
    reset_text(url)
}

pub fn withdraw_cancel_text(url: &str) -> String {
    format!(
        "계정 삭제가 예약되었습니다. 14일 뒤 이름·이메일과 개인 워크스페이스가 정리됩니다. 삭제를 원하지 않으면 기한 전에 아래 링크에서 취소하거나 관리자에게 요청하세요. 링크를 여는 것만으로는 취소되지 않습니다.\n{url}"
    )
}

pub fn reset_text(url: &str) -> String {
    format!(
        "{url}\n{MAGIC_TTL_MINUTES}분 안에 사용할 수 있습니다. 본인이 요청하지 않았다면 무시하세요."
    )
}

pub fn comment_text(actor: &str, body: &str) -> String {
    format!("{actor}님이 댓글을 남겼습니다.\n\n{body}")
}

pub fn digest_text(count: i64) -> String {
    format!("읽지 않은 알림이 {count}건 있습니다.")
}

pub fn identity_linked_text(provider: &str) -> String {
    format!(
        "이 계정에 소셜 로그인({provider})이 연결되었습니다.\n\n본인이 연결한 것이 아니라면 계정 설정에서 연결을 해제하고 비밀번호를 변경하세요."
    )
}

pub fn identity_unlinked_text(provider: &str) -> String {
    format!(
        "이 계정에서 소셜 로그인({provider}) 연결이 해제되었습니다.\n\n본인이 해제한 것이 아니라면 계정 설정을 확인하고 비밀번호를 변경하세요."
    )
}
