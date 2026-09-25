//! Korean copy matches packages/i18n/src/locales/ko.json at the source SHA.

pub const INVITE_SUBJECT: &str = "워크스페이스 초대";
pub const RESET_SUBJECT: &str = "FVOCI 비밀번호 재설정";
pub const COMMENT_SUBJECT: &str = "새 댓글이 있습니다";
pub const DIGEST_SUBJECT: &str = "알림 다이제스트";
pub const IDENTITY_LINKED_SUBJECT: &str = "[FVOCI] 소셜 로그인 연결 알림";
pub const IDENTITY_UNLINKED_SUBJECT: &str = "[FVOCI] 소셜 로그인 연결 해제 알림";

pub const MAGIC_TTL_MINUTES: i64 = 15;

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
