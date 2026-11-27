//! The credentials of a request as HTTP carries them: the session token in the
//! `Authorization` header or the session cookie, the `Set-Cookie` header that gives or
//! takes it, and the `Origin` header. What they say is `crate::auth::token`'s to read;
//! this takes their text from a request's headers and puts a cookie into a response's.

use axum::http::header::{AUTHORIZATION, COOKIE, ORIGIN};
use axum::http::{HeaderMap, HeaderValue};

use crate::auth::token::{self, Credential, Origins, SessionCookie};

/// The value of the request's `Authorization` header, if it is text.
fn authorization(headers: &HeaderMap) -> Option<&str> {
    headers.get(AUTHORIZATION)?.to_str().ok()
}

/// The lines of the request's `Cookie` header that are text.
fn cookie_lines(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
}

/// Returns the token of an `Authorization: Bearer <token>` header
/// ([`token::bearer_from_header`]).
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    authorization(headers).and_then(token::bearer_from_header)
}

/// Returns the value of the named cookie, if the request carries it.
pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    token::cookie_from_headers(cookie_lines(headers), name)
}

/// Returns the session token a request carries, and how
/// ([`SessionCookie::credential_from_parts`]).
pub(crate) fn session_credential(
    cookie: &SessionCookie,
    headers: &HeaderMap,
) -> Option<Credential> {
    cookie.credential_from_parts(authorization(headers), cookie_lines(headers))
}

/// The `Set-Cookie` header that gives a browser `token`.
pub(crate) fn set_cookie_header(cookie: &SessionCookie, token: &str) -> HeaderValue {
    header_value(cookie.set_cookie_value(token))
}

/// The `Set-Cookie` header that makes a browser forget the token.
pub(crate) fn clear_cookie_header(cookie: &SessionCookie) -> HeaderValue {
    header_value(cookie.clear_cookie_value())
}

fn header_value(cookie: String) -> HeaderValue {
    // A token is base64url text and the rest is fixed: always a valid header value.
    HeaderValue::try_from(cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Returns the `Origin` header of a request if it names an origin that `origins` does
/// not allow; one that is no text is no allowed one.
pub(crate) fn foreign_origin<'h>(
    origins: &Origins,
    headers: &'h HeaderMap,
) -> Option<&'h HeaderValue> {
    headers.get(ORIGIN).filter(|origin| {
        !origin
            .to_str()
            .is_ok_and(|origin| origins.allows_origin(origin))
    })
}

#[cfg(test)]
mod tests {
    use crate::auth::token::Https;

    use super::*;

    fn headers(pairs: &[(&'static str, &[u8])]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                *name,
                HeaderValue::from_bytes(value).expect("a header value"),
            );
        }
        headers
    }

    #[test]
    fn the_credential_is_read_from_the_headers_that_are_text() {
        let plain = SessionCookie::new(Https::No, 60);
        let both = headers(&[
            ("cookie", b"a=\xff"),
            ("cookie", b"fvoci_session=of-the-cookie"),
            ("authorization", b"Bearer of-the-header"),
        ]);
        assert_eq!(bearer_token(&both), Some("of-the-header"));
        assert_eq!(
            session_credential(&plain, &both),
            Some(Credential::Bearer("of-the-header".to_owned()))
        );
        let cookie_alone = headers(&[
            ("authorization", b"Bearer \xff"),
            ("cookie", b"fvoci_session=of-the-cookie"),
        ]);
        assert_eq!(bearer_token(&cookie_alone), None, "no text, no token");
        assert_eq!(
            session_credential(&plain, &cookie_alone),
            Some(Credential::Cookie("of-the-cookie".to_owned()))
        );
        assert_eq!(
            cookie_value(&cookie_alone, "fvoci_session").as_deref(),
            Some("of-the-cookie")
        );
        assert_eq!(cookie_value(&cookie_alone, "other"), None);
    }

    #[test]
    fn the_cookie_headers_are_what_the_cookie_says() {
        let secure = SessionCookie::new(Https::Yes, 60);
        assert_eq!(
            set_cookie_header(&secure, "tok"),
            secure.set_cookie_value("tok").as_str()
        );
        assert_eq!(
            clear_cookie_header(&secure),
            secure.clear_cookie_value().as_str()
        );
    }

    /// The origin as the `http` crate's URI reads it: what the server compared before it
    /// read origins itself.
    fn origin_by_uri(url: &str) -> Option<String> {
        let uri: axum::http::Uri = url.trim().parse().ok()?;
        let scheme = uri.scheme_str()?.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let host = uri
            .host()
            .filter(|host| !host.is_empty())?
            .to_ascii_lowercase();
        Some(match uri.port_u16() {
            Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
            _ => format!("{scheme}://{host}"),
        })
    }

    #[test]
    fn origins_are_read_as_the_uri_of_http_reads_them() {
        for url in [
            "https://boards.example.com",
            "HTTPS://Boards.Example.COM:443/boards?x#y",
            "http://localhost:5173",
            "http://127.0.0.1:8000",
            "http://[::1]:8000",
            "http://[::1]",
            "https://[2001:DB8::1]:443",
            "http://[::ffff:127.0.0.1]:8080",
            "https://someone:secret@example.com/x",
            "https://example.com:",
            "http://example.com:0",
            "http://example.com:65535",
            "https://a b@example.com",
            "https://example.com/a b",
            "https://example.com/\u{e4}",
            "https://exa mple.com",
            "http://exa\tmple.com",
            "https://\u{4f8b}.jp",
            "https://",
            "https://:443",
            "https://someone@",
            "http://[::1",
            "null",
            "",
            "example.com",
            "localhost:8000",
            "ftp://example.com",
            "file:///boards",
            "/boards",
            "*",
        ] {
            assert_eq!(token::origin_of(url), origin_by_uri(url), "{url:?}");
        }
        // Where the URI read a port or an address that is none as no port, or as a host,
        // there is no origin.
        for url in [
            "http://example.com:65536",
            "http://example.com:+80",
            "http://example.com:x",
            "http://[zz::1]:80",
            "http://[::1]8000",
        ] {
            assert!(origin_by_uri(url).is_some(), "{url:?}");
            assert_eq!(token::origin_of(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_request_without_an_origin_header_names_no_foreign_origin() {
        let origins = Origins::new("http://localhost:8000", &[]).expect("valid");
        assert_eq!(foreign_origin(&origins, &headers(&[])), None);
        assert_eq!(
            foreign_origin(&origins, &headers(&[("origin", b"http://localhost:8000")])),
            None
        );
        for origin in [
            &b"https://evil.example"[..],
            b"null",
            b"http://localhost:8000\xff",
        ] {
            assert!(
                foreign_origin(&origins, &headers(&[("origin", origin)])).is_some(),
                "{origin:?}: a page without an allowed origin is a foreign one"
            );
        }
    }
}
