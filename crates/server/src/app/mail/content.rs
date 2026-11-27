//! Plain-text and HTML messages containing only public links and human-facing text.

use super::SecretLink;
use crate::app::language::Language;

/// The text of one message, in one language.
pub(crate) struct Content {
    pub(crate) subject: &'static str,
    pub(crate) plain: String,
    pub(crate) html: String,
    pub(crate) language: Language,
}

impl Content {
    pub(crate) fn reset(link: &SecretLink, language: Language) -> Self {
        let (subject, intro, ending) = match language {
            Language::English => (
                "Reset your fvoci password",
                "Use this link to choose a new password:",
                "This link expires in one hour and can be used once. If you did not ask for it, ignore this message.",
            ),
            Language::Korean => (
                "fvoci 비밀번호 재설정",
                "다음 링크에서 새 비밀번호를 설정하세요:",
                "이 링크는 1시간 동안 한 번만 쓸 수 있습니다. 요청하지 않았다면 이 메일을 무시하세요.",
            ),
        };
        Self::link(subject, intro, ending, link.as_str(), language)
    }

    /// The link that confirms the address of a registration, which signs its person in
    /// as `username`: named, so that who did not sign up sees whose registration it is.
    pub(crate) fn verification(link: &SecretLink, username: &str, language: Language) -> Self {
        let (subject, intro, ending) = match language {
            Language::English => (
                "Confirm your email address for fvoci",
                format!("Use this link to confirm your email address and sign in as {username}:"),
                "This link expires in 24 hours and can be used once. In another browser than the one you signed up in, it asks for the password you chose. If you did not sign up, ignore this message: the sign-up is removed after seven days.",
            ),
            Language::Korean => (
                "fvoci 이메일 주소 확인",
                format!("다음 링크로 이메일 주소를 확인하고 {username}(으)로 로그인하세요:"),
                "이 링크는 24시간 동안 한 번만 쓸 수 있습니다. 가입한 브라우저가 아닌 곳에서 열면 가입할 때 정한 비밀번호를 묻습니다. 가입한 적이 없으면 이 메일을 무시하면 됩니다. 확인하지 않은 가입은 7일 뒤 지워집니다.",
            ),
        };
        Self::link(subject, &intro, ending, link.as_str(), language)
    }

    /// The link that makes the address it is mailed to the address of the account
    /// `username`: named, so that who did not ask sees whose account it is.
    pub(crate) fn email_change(link: &SecretLink, username: &str, language: Language) -> Self {
        let (subject, intro, ending) = match language {
            Language::English => (
                "Confirm your new email address for fvoci",
                format!(
                    "Use this link to make this the email address of the fvoci account {username}:"
                ),
                "This link expires in 24 hours and can be used once. Until it is used, the account keeps the address it has. If you did not ask for this, ignore this message: nothing changes.",
            ),
            Language::Korean => (
                "fvoci 새 이메일 주소 확인",
                format!("다음 링크로 이 주소를 fvoci 계정 {username}의 이메일 주소로 정하세요:"),
                "이 링크는 24시간 동안 한 번만 쓸 수 있습니다. 쓰기 전까지 계정의 주소는 그대로입니다. 요청한 적이 없으면 이 메일을 무시하면 됩니다. 아무것도 바뀌지 않습니다.",
            ),
        };
        Self::link(subject, &intro, ending, link.as_str(), language)
    }

    /// The notice to the address the account `username` had, that it is the account's
    /// no more, with the link that gives the account that address back (`link`), for
    /// a change its owner did not make. It names nothing of the new address: whoever
    /// reads the old mailbox learns that the address changed, not where to.
    pub(crate) fn email_changed(link: &SecretLink, username: &str, language: Language) -> Self {
        let (subject, intro, ending) = match language {
            Language::English => (
                "The email address of your fvoci account was changed",
                format!(
                    "The email address of the fvoci account {username} was changed. This address is no longer the account's: mail for it, a password reset link too, goes to the new address from now on. If you did not change it, somebody else knows the account's password: this link gives the account this address back and signs it out everywhere:"
                ),
                "The link works for seven days and once, and opening it changes nothing until you press the button on its page. If you changed the address yourself, ignore this message.",
            ),
            Language::Korean => (
                "fvoci 계정의 이메일 주소가 바뀌었습니다",
                format!(
                    "fvoci 계정 {username}의 이메일 주소가 바뀌었습니다. 이 주소는 더 이상 계정의 주소가 아닙니다. 비밀번호 재설정 링크를 비롯한 계정의 메일은 이제 새 주소로 갑니다. 직접 바꾸지 않았다면 다른 사람이 계정의 비밀번호를 알고 있는 것입니다. 다음 링크로 계정의 주소를 이 주소로 되돌리고 모든 기기에서 로그아웃하세요:"
                ),
                "이 링크는 7일 동안 한 번만 쓸 수 있고, 링크를 열기만 해서는 아무것도 바뀌지 않습니다. 열린 페이지의 단추를 눌러야 합니다. 직접 바꿨다면 이 메일을 무시하세요.",
            ),
        };
        Self::link(subject, &intro, ending, link.as_str(), language)
    }

    /// The notice to the address the account `username` had until a revert link mailed
    /// to an earlier address was used. It names nothing of the address the account has
    /// now (nor whether the earlier address was restored or cleared), and carries no link.
    pub(crate) fn email_reverted(username: &str, language: Language) -> Self {
        let (subject, said, ending) = match language {
            Language::English => (
                "The email address of your fvoci account was changed back",
                format!(
                    "A link mailed to an earlier address of the fvoci account {username} was used to undo an email change. This address is no longer the account's, the account was signed out everywhere, and its password works no more until it is reset."
                ),
                "If you did not ask for this, whoever reads the account's earlier address did: ask an administrator of your organization, or the operator of the server, for help.",
            ),
            Language::Korean => (
                "fvoci 계정의 이메일 주소 변경이 되돌려졌습니다",
                format!(
                    "예전 주소로 보낸 링크로 fvoci 계정 {username}의 이메일 주소 변경이 되돌려졌습니다. 이 주소는 더 이상 계정의 주소가 아니고, 계정은 모든 기기에서 로그아웃되었으며, 비밀번호는 재설정할 때까지 쓸 수 없습니다."
                ),
                "요청한 적이 없다면 계정의 예전 주소를 읽는 사람이 한 것입니다. 조직의 관리자나 서버 운영자에게 도움을 요청하세요.",
            ),
        };
        let tag = language.tag();
        Self {
            subject,
            plain: format!("{said}\n\n{ending}\n"),
            html: format!(
                "<!doctype html><html lang=\"{tag}\"><body><p>{}</p><p>{}</p></body></html>",
                escape(&said),
                escape(ending)
            ),
            language,
        }
    }

    pub(crate) fn invitation(
        link: &str,
        language: Language,
        role: fvoci_model::WorkspaceRole,
    ) -> Self {
        let (subject, intro) = match language {
            Language::English => (
                "You are invited to fvoci",
                "Use this invitation link to join:",
            ),
            Language::Korean => ("fvoci 초대", "다음 초대 링크로 참여하세요:"),
        };
        let ending = match (language, role) {
            (Language::English, fvoci_model::WorkspaceRole::Guest) => {
                "This link is yours alone: it brings you to one board or wiki and can be used once. It ends when the invitation does."
            }
            (Language::Korean, fvoci_model::WorkspaceRole::Guest) => {
                "이 링크는 받는 분만을 위한 것으로, 보드나 위키 하나에 한 번 들어올 수 있습니다. 초대가 끝나면 함께 끝납니다."
            }
            (
                Language::English,
                fvoci_model::WorkspaceRole::Admin | fvoci_model::WorkspaceRole::Member,
            ) => "You join as a member. The link keeps the expiry it was made with.",
            (
                Language::Korean,
                fvoci_model::WorkspaceRole::Admin | fvoci_model::WorkspaceRole::Member,
            ) => "구성원으로 참여하는 링크입니다. 원래 만료일을 유지합니다.",
        };
        Self::link(subject, intro, ending, link, language)
    }

    fn link(
        subject: &'static str,
        intro: &str,
        ending: &str,
        link: &str,
        language: Language,
    ) -> Self {
        let tag = language.tag();
        let safe = escape(link);
        Self {
            subject,
            plain: format!("{intro}\n\n{link}\n\n{ending}\n"),
            html: format!(
                "<!doctype html><html lang=\"{tag}\"><body><p>{}</p><p><a href=\"{safe}\">{safe}</a></p><p>{}</p></body></html>",
                escape(intro),
                escape(ending)
            ),
            language,
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mail::SecretPage;

    /// The link to `page` of a server at `https://example.test` with `secret`.
    fn secret_link(page: SecretPage, secret: &str) -> SecretLink {
        let server: crate::config::ServerUrl = "https://example.test".parse().expect("a URL");
        SecretLink::new(&server, page, secret)
    }

    #[test]
    fn the_first_language_the_mail_is_written_in_is_taken_or_else_english() {
        for (tags, want) in [
            (&["ko", "en"][..], Language::Korean),
            (&["auto", "ko-KR"][..], Language::Korean),
            (&["ja", "KO"][..], Language::Korean),
            (&["ja", "auto", "EN-gb"][..], Language::English),
            (
                &["ja", "zh-Hans", "", "ko_KR", " ko"][..],
                Language::English,
            ),
            (&["en", "ko"][..], Language::English),
            (&[][..], Language::English),
        ] {
            let choices = tags.iter().map(|tag| Language::written_in(tag));
            assert_eq!(Language::first_of(choices), want, "{tags:?}");
        }
    }

    #[test]
    fn reset_messages_have_both_languages_and_escape_html() {
        for (language, subject) in [
            (Language::English, "Reset your fvoci password"),
            (Language::Korean, "fvoci 비밀번호 재설정"),
        ] {
            let content = Content::reset(
                &secret_link(SecretPage::ResetPassword, "secret\"<&"),
                language,
            );
            assert_eq!(content.subject, subject);
            assert!(
                content
                    .plain
                    .contains("https://example.test/reset-password#")
            );
            assert!(content.html.contains("&quot;&lt;&amp;"));
            assert!(!content.html.contains("secret\"<&"));
        }
    }

    #[test]
    fn verification_messages_have_both_languages_and_name_no_workspace() {
        for (language, subject) in [
            (Language::English, "Confirm your email address for fvoci"),
            (Language::Korean, "fvoci 이메일 주소 확인"),
        ] {
            let content =
                Content::verification(&secret_link(SecretPage::VerifyEmail, "t<"), "a<b", language);
            assert_eq!(content.subject, subject);
            assert!(
                content
                    .plain
                    .contains("https://example.test/verify-email#t<")
            );
            assert!(content.html.contains("#t&lt;"), "{}", content.html);
            assert!(
                content.plain.contains("a<b"),
                "the user name: {}",
                content.plain
            );
            assert!(content.html.contains("a&lt;b"), "escaped: {}", content.html);
            let said = match language {
                Language::English => [
                    "In another browser than the one you signed up in, it asks for the password you chose.",
                    "If you did not sign up, ignore this message",
                ],
                Language::Korean => [
                    "가입한 브라우저가 아닌 곳에서 열면 가입할 때 정한 비밀번호를 묻습니다.",
                    "가입한 적이 없으면 이 메일을 무시하면 됩니다",
                ],
            };
            for sentence in said {
                assert!(content.plain.contains(sentence), "{}", content.plain);
            }
            assert!(
                content
                    .html
                    .contains(&format!("lang=\"{}\"", language.tag()))
            );
            for text in [content.subject, &content.plain, &content.html] {
                let text = text.to_lowercase();
                for word in UNSAID {
                    assert!(!text.contains(word), "{language:?}: {word} in {text}");
                }
            }
        }
    }

    #[test]
    fn the_mails_of_a_changed_address_have_both_languages_and_name_no_workspace() {
        for language in [Language::English, Language::Korean] {
            let link = Content::email_change(
                &secret_link(SecretPage::ConfirmEmail, "t<"),
                "a<b",
                language,
            );
            assert!(
                link.plain.contains("https://example.test/confirm-email#t<"),
                "{}",
                link.plain
            );
            assert!(link.html.contains("#t&lt;"), "{}", link.html);
            let notice = Content::email_changed(
                &secret_link(SecretPage::RevertEmail, "r<"),
                "a<b",
                language,
            );
            assert!(
                notice
                    .plain
                    .contains("https://example.test/revert-email#r<"),
                "the link back: {}",
                notice.plain
            );
            assert!(!notice.plain.contains('@'), "no address: {}", notice.plain);
            let reverted = Content::email_reverted("a<b", language);
            assert!(
                !reverted.plain.contains("http"),
                "no link: {}",
                reverted.plain
            );
            assert!(
                !reverted.plain.contains('@'),
                "no address: {}",
                reverted.plain
            );
            for content in [&link, &notice, &reverted] {
                assert!(content.plain.contains("a<b"), "{}", content.plain);
                assert!(content.html.contains("a&lt;b"), "escaped: {}", content.html);
                assert!(
                    content
                        .html
                        .contains(&format!("lang=\"{}\"", language.tag()))
                );
                for text in [content.subject, &content.plain, &content.html] {
                    let text = text.to_lowercase();
                    for word in ["workspace", "워크스페이스", "my space", "내 공간"] {
                        assert!(!text.contains(word), "{language:?}: {word} in {text}");
                    }
                }
            }
        }
    }

    /// Words an invitation does not use: until someone has signed up and works with
    /// others, the product does not name the workspace (the organization it may).
    const UNSAID: [&str; 2] = ["workspace", "워크스페이스"];

    #[test]
    fn invitations_name_no_workspace_in_either_language() {
        use fvoci_model::WorkspaceRole;
        for language in [Language::English, Language::Korean] {
            for role in [
                WorkspaceRole::Admin,
                WorkspaceRole::Member,
                WorkspaceRole::Guest,
            ] {
                let content = Content::invitation("https://example.test/invite/t", language, role);
                for text in [content.subject, &content.plain, &content.html] {
                    let text = text.to_lowercase();
                    for word in UNSAID {
                        assert!(
                            !text.contains(word),
                            "{language:?} {role:?}: {word} in {text}"
                        );
                    }
                }
            }
        }
    }
}
