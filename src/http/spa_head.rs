//! SPA shell `<head>` injection for link unfurlers (source
//! `apps/server/src/spa-html.ts` `injectShareOg`).
//!
//! Only the shell's `<title>` and five `<meta>` tags change: people still get
//! the SPA, unfurl bots read the head. Nothing inline is added, so the shell's
//! build-time inline script/style hashes (the global CSP) stay valid.

/// Values for one `/s/{token}` shell. `title`/`excerpt` are already one line
/// (source `foldOneLine`); every value is escaped here.
pub struct ShareOgMeta<'a> {
    pub title: &'a str,
    pub excerpt: &'a str,
    pub url: &'a str,
}

/// Source `Bun.escapeHTML`: `& < > " '`.
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Source `withHeadTags`: the first `<title>…</title>` (no nested `<`) gets the
/// title, and the tags go before the first `</head>`. Plain string splicing,
/// so `$&`-like text in a title is literal.
fn with_head_tags(html: &str, title: &str, tags: &str) -> String {
    let mut out = html.to_string();
    let mut from = 0;
    while let Some(offset) = out[from..].find("<title>") {
        let start = from + offset;
        let body_start = start + "<title>".len();
        match out[body_start..].find('<') {
            Some(len) if out[body_start + len..].starts_with("</title>") => {
                let end = body_start + len + "</title>".len();
                out.replace_range(start..end, &format!("<title>{title}</title>"));
                break;
            }
            Some(_) => from = body_start,
            None => break,
        }
    }
    if let Some(at) = out.find("</head>") {
        out.insert_str(at, tags);
    }
    out
}

/// Source `injectShareOg`. There is no `og:image`: the app ships no image
/// asset for it.
pub fn inject_share_og(html: &str, meta: &ShareOgMeta<'_>) -> String {
    let title = escape_html(meta.title);
    let description = escape_html(meta.excerpt);
    let url = escape_html(meta.url);
    let tags = format!(
        "<meta property=\"og:title\" content=\"{title}\"/><meta property=\"og:description\" content=\"{description}\"/><meta property=\"og:url\" content=\"{url}\"/><meta property=\"og:type\" content=\"article\"/><meta name=\"twitter:card\" content=\"summary\"/>"
    );
    with_head_tags(html, &title, &tags)
}

/// Source `SHARE_PATH` (`^/s/([^/]+)/?$`): the share root only, not
/// `/s/{token}/attachments/...`.
pub fn share_shell_token(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/s/")?;
    let token = rest.strip_suffix('/').unwrap_or(rest);
    (!token.is_empty() && !token.contains('/')).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: &str =
        "<!DOCTYPE html><html><head><title>FVOCI</title></head><body></body></html>";

    #[test]
    fn head_injection_keeps_the_body_byte_for_byte() {
        let body = "<body class=\"app\">\n  <div id=\"root\">한글 &amp; 공백</div>\n  <svg><title>본문 아이콘</title></svg>\n  <script type=\"module\">const marker = \"</head> $& $1\";</script>\n</body></html>";
        let html = SHELL.replace("<body></body></html>", body);
        let out = inject_share_og(
            &html,
            &ShareOgMeta {
                title: "공유 제목",
                excerpt: "공유 설명",
                url: "https://example.test/s/shared",
            },
        );
        assert_eq!(&out[out.find("<body").unwrap()..], body);
        assert!(out.contains("<title>공유 제목</title>"));
        assert!(out.contains("<meta property=\"og:description\" content=\"공유 설명\"/>"));
        assert!(out.contains("<meta name=\"twitter:card\" content=\"summary\"/></head>"));
    }

    #[test]
    fn title_description_and_url_are_escaped() {
        let out = inject_share_og(
            SHELL,
            &ShareOgMeta {
                title: "x</title><script>",
                excerpt: "\" onclick=\"x",
                url: "https://example.test/?q=\"",
            },
        );
        assert!(out.contains("<title>x&lt;/title&gt;&lt;script&gt;</title>"));
        assert!(!out.contains("</title><script>"));
        assert!(out.contains("content=\"&quot; onclick=&quot;x\""));
        assert!(out.contains("content=\"https://example.test/?q=&quot;\""));
        assert!(!out.contains("<script"));
        let quote = inject_share_og(
            SHELL,
            &ShareOgMeta {
                title: "it's $& $1",
                excerpt: "",
                url: "u",
            },
        );
        assert!(quote.contains("<title>it&#x27;s $&amp; $1</title>"));
    }

    #[test]
    fn share_shell_token_matches_the_root_only() {
        assert_eq!(share_shell_token("/s/abc"), Some("abc"));
        assert_eq!(share_shell_token("/s/abc/"), Some("abc"));
        assert_eq!(share_shell_token("/s/abc/attachments/x"), None);
        assert_eq!(share_shell_token("/s/"), None);
        assert_eq!(share_shell_token("/s//"), None);
        assert_eq!(share_shell_token("/share/abc"), None);
    }
}
