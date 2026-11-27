//! How a request says who sends it, and from where: the session token in a header or in
//! the session cookie, and the origin of the page that sent it.
//!
//! A browser gets the token in a cookie that scripts cannot read and sends it back by
//! itself. That is why a request that comes with the cookie must show that the server's
//! own pages sent it (see [`Origins`] and the request-forgery header in `crate::api`).
//! A client that is no browser reads the token from the `Set-Cookie` header of the login
//! and sends it in the `Authorization` header, which no browser adds by itself.
//!
//! What the headers say is read here from their text; taking that text from a request,
//! and putting a cookie into a response, is HTTP's (`crate::http_credentials`).

use std::net::Ipv6Addr;

/// The session token of a request, by how the request carries it.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// In the `Authorization` header, as `Bearer <token>`: sent on purpose.
    Bearer(String),
    /// In the session cookie: sent by a browser, whether or not the page meant to.
    Cookie(String),
}

impl Credential {
    /// The token, wherever the request carried it.
    pub fn token(&self) -> &str {
        match self {
            Self::Bearer(token) | Self::Cookie(token) => token,
        }
    }
}

impl std::fmt::Debug for Credential {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bearer(_) => "Credential::Bearer(..)",
            Self::Cookie(_) => "Credential::Cookie(..)",
        })
    }
}

/// Returns the token of an `Authorization` header whose value is `Bearer <token>`, the
/// scheme in any letter case. A header of another scheme is not a session token.
pub fn bearer_from_header(authorization: &str) -> Option<&str> {
    // The scheme alone is a bearer token too, an empty one: whoever sends the header
    // means to present a token, and is not to be taken for a browser with a cookie.
    let (scheme, token) = authorization.split_once(' ').unwrap_or((authorization, ""));
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// The session cookie: its name and attributes, which depend on whether the server is
/// reached over HTTPS, and how long a browser keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCookie {
    secure: bool,
    max_age_seconds: i64,
}

impl SessionCookie {
    /// The name of the cookie where the server is reached over HTTPS. The `__Host-`
    /// prefix makes browsers accept it only as a `Secure` cookie of exactly this host
    /// with `Path=/`, so that no other host of the domain can plant one.
    pub const SECURE_NAME: &'static str = "__Host-fvoci_session";
    /// The name of the cookie where the server is reached over plain HTTP, as in
    /// development: browsers refuse the prefixed name there.
    pub const PLAIN_NAME: &'static str = "fvoci_session";

    /// A session cookie that a browser keeps for `max_age_seconds`, `Secure` and with
    /// the prefixed name if the server is reached over HTTPS.
    pub fn new(https: Https, max_age_seconds: i64) -> Self {
        Self {
            secure: https == Https::Yes,
            max_age_seconds,
        }
    }

    /// The name of the cookie.
    pub fn name(&self) -> &'static str {
        if self.secure {
            Self::SECURE_NAME
        } else {
            Self::PLAIN_NAME
        }
    }

    /// Returns the session token of a request whose `Authorization` header is
    /// `authorization` and whose `Cookie` header has the lines `cookies`, and how it
    /// carries it. A token in the `Authorization` header counts before one in the
    /// cookie: it was sent on purpose.
    pub fn credential_from_parts<'line>(
        &self,
        authorization: Option<&str>,
        cookies: impl IntoIterator<Item = &'line str>,
    ) -> Option<Credential> {
        if let Some(token) = authorization.and_then(bearer_from_header) {
            return Some(Credential::Bearer(token.to_owned()));
        }
        cookie_from_headers(cookies, self.name())
            .filter(|token| !token.is_empty())
            .map(Credential::Cookie)
    }

    /// The value of the `Set-Cookie` header that gives a browser the token: not readable
    /// by scripts, sent along with requests from the same site only, for the whole host.
    pub fn set_cookie_value(&self, token: &str) -> String {
        self.cookie(token, self.max_age_seconds)
    }

    /// The value of the `Set-Cookie` header that makes a browser forget the token.
    pub fn clear_cookie_value(&self) -> String {
        self.cookie("", 0)
    }

    fn cookie(&self, token: &str, max_age_seconds: i64) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!(
            "{}={token}; Max-Age={max_age_seconds}; Path=/; HttpOnly; SameSite=Lax{secure}",
            self.name()
        )
    }
}

/// Whether the server is reached over HTTPS, which its `server_url` says: behind a
/// proxy that ends TLS the server itself listens on plain HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Https {
    /// `server_url` begins with `https://`.
    Yes,
    /// `server_url` begins with `http://`.
    No,
}

/// The origins whose pages may send requests that change something, and may open
/// websocket connections: the server's own, from `server_url`, and for development the
/// ones listed in `allowed_origins`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origins {
    allowed: Vec<String>,
    https: Https,
}

impl Origins {
    /// Reads the origin of `server_url` and of each of `allowed_origins`.
    ///
    /// # Errors
    ///
    /// Returns the first of them that is no `http` or `https` URL with a host.
    pub fn new(server_url: &str, allowed_origins: &[String]) -> Result<Self, String> {
        let own = origin_of(server_url)
            .ok_or_else(|| format!("server_url {server_url:?} is no http or https URL"))?;
        let https = if own.starts_with("https://") {
            Https::Yes
        } else {
            Https::No
        };
        let mut allowed = vec![own];
        for origin in allowed_origins {
            allowed.push(origin_of(origin).ok_or_else(|| {
                format!("allowed_origins has {origin:?}, which is no http or https origin")
            })?);
        }
        Ok(Self { allowed, https })
    }

    /// Whether the server's own origin is an HTTPS one.
    pub fn https(&self) -> Https {
        self.https
    }

    /// Whether a page of `origin`, as a request names it in its `Origin` header, may
    /// send it. A request without the header names none and is not judged here.
    pub fn allows_origin(&self, origin: &str) -> bool {
        origin_of(origin).is_some_and(|origin| self.allowed.contains(&origin))
    }
}

/// The origin of a URL in one spelling (RFC 6454, 6.1): scheme and host in lower case,
/// the port unless it is the scheme's own. `None` for what is no `http` or `https` URL
/// with a host; the `null` that browsers send for pages without an origin is none.
/// Who signs in (`user@`) is no part of an origin, nor is the path.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let url = url.trim();
    // What a URI may hold of ASCII (RFC 3986, 2): its unreserved and reserved
    // characters, and `%`; past ASCII, the letters of a path written out (RFC 3987).
    let in_a_uri = |byte: u8| {
        !byte.is_ascii()
            || byte.is_ascii_alphanumeric()
            || b"-._~:/?#[]@!$&'()*+,;=%".contains(&byte)
    };
    if !url.bytes().all(in_a_uri) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let place = authority
        .rsplit_once('@')
        .map_or(authority, |(_, place)| place);
    let (host, port) = host_and_port(place)?;
    Some(match port {
        Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    })
}

/// The host of an authority in lower case, an IPv6 address in its brackets, and its
/// port if it names one (RFC 3986, 3.2.2 and 3.2.3). `None` for an empty or unreadable
/// host or a port that is no number of 16 bits.
fn host_and_port(place: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = if let Some(bracketed) = place.strip_prefix('[') {
        let (address, after) = bracketed.split_once(']')?;
        address.parse::<Ipv6Addr>().ok()?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':')?),
        };
        (format!("[{address}]"), port)
    } else {
        let (host, port) = match place.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (place, None),
        };
        let readable = |byte: u8| byte.is_ascii_alphanumeric() || b"-._~%".contains(&byte);
        if host.is_empty() || !host.bytes().all(readable) {
            return None;
        }
        (host.to_owned(), port)
    };
    let port = match port {
        None | Some("") => None,
        Some(digits) if digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(digits.parse().ok()?)
        }
        Some(_) => return None,
    };
    Some((host.to_ascii_lowercase(), port))
}

/// Returns the value of the cookie `name` among the lines of a request's `Cookie`
/// header, if one of them carries it.
pub fn cookie_from_headers<'line>(
    lines: impl IntoIterator<Item = &'line str>,
    name: &str,
) -> Option<String> {
    lines
        .into_iter()
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(
        cookie: &SessionCookie,
        authorization: Option<&str>,
        cookies: &[&str],
    ) -> Option<Credential> {
        cookie.credential_from_parts(authorization, cookies.iter().copied())
    }

    #[test]
    fn the_token_is_read_from_the_bearer_header_or_the_cookie_of_this_mode() {
        let plain = SessionCookie::new(Https::No, 60);
        let secure = SessionCookie::new(Https::Yes, 60);
        let bearer = |token: &str| Some(Credential::Bearer(token.to_owned()));
        let cookie = |token: &str| Some(Credential::Cookie(token.to_owned()));

        assert_eq!(read(&plain, None, &[]), None, "nothing");
        for header in ["Bearer mytoken", "BEARER mytoken", "bearer  mytoken "] {
            assert_eq!(
                read(&plain, Some(header), &[]),
                bearer("mytoken"),
                "{header}"
            );
        }
        // A header of the scheme without a token is a credential, and no good one.
        for header in ["Bearer ", "Bearer", "bearer"] {
            assert_eq!(read(&plain, Some(header), &[]), bearer(""), "{header:?}");
        }
        // Non-Bearer authorization schemes are not accepted as session tokens.
        for header in ["token mytoken", "Basic dTpw", "Bearer:mytoken", "mytoken"] {
            assert_eq!(read(&plain, Some(header), &[]), None, "{header}");
        }

        assert_eq!(
            read(&plain, None, &["a=b; fvoci_session=mytoken; c=d"]),
            cookie("mytoken")
        );
        assert_eq!(
            read(&plain, None, &["a=b", "fvoci_session=\"mytoken\""]),
            cookie("mytoken"),
            "several lines, a quoted value"
        );
        assert_eq!(
            read(&secure, None, &["__Host-fvoci_session=mytoken"]),
            cookie("mytoken")
        );
        // Each mode reads its own name only: a cookie under the plain name could be
        // planted over HTTP, and the prefixed one is not set without HTTPS.
        assert_eq!(read(&secure, None, &["fvoci_session=mytoken"]), None);
        assert_eq!(read(&plain, None, &["__Host-fvoci_session=mytoken"]), None);
        assert_eq!(
            read(&plain, None, &["fvoci_session="]),
            None,
            "an empty cookie is none"
        );

        // Both: the header, which was sent on purpose.
        assert_eq!(
            read(
                &plain,
                Some("Bearer of-the-header"),
                &["fvoci_session=of-the-cookie"]
            ),
            bearer("of-the-header")
        );
    }

    #[test]
    fn the_cookie_is_http_only_same_site_and_secure_over_https() {
        assert_eq!(
            SessionCookie::new(Https::No, 1_209_600).set_cookie_value("tok"),
            "fvoci_session=tok; Max-Age=1209600; Path=/; HttpOnly; SameSite=Lax"
        );
        assert_eq!(
            SessionCookie::new(Https::Yes, 1_209_600).set_cookie_value("tok"),
            "__Host-fvoci_session=tok; Max-Age=1209600; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
        assert_eq!(
            SessionCookie::new(Https::No, 1_209_600).clear_cookie_value(),
            "fvoci_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax"
        );
        assert_eq!(
            SessionCookie::new(Https::Yes, 60).clear_cookie_value(),
            "__Host-fvoci_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }

    #[test]
    fn the_token_is_not_in_the_debug_form_of_a_credential() {
        let debug = format!("{:?}", Credential::Cookie("SECRET".to_owned()));
        assert!(!debug.contains("SECRET"), "{debug}");
    }

    #[test]
    fn origins_are_compared_by_scheme_host_and_port() {
        let origins = Origins::new(
            "https://Boards.Example.com/boards",
            &[
                "http://localhost:5173".to_owned(),
                "http://[::1]:8000".to_owned(),
            ],
        )
        .expect("valid origins");
        assert_eq!(origins.https(), Https::Yes);

        for origin in [
            "https://boards.example.com",
            "https://BOARDS.example.com",
            "https://boards.example.com:443",
            "http://localhost:5173",
            "http://[::1]:8000",
        ] {
            assert!(origins.allows_origin(origin), "{origin}");
        }
        for origin in [
            "http://boards.example.com",
            "https://boards.example.com:8443",
            "https://evil.example.com",
            "https://boards.example.com.evil.example",
            "https://example.com",
            "http://localhost",
            "http://localhost:5174",
            "http://[::1]:8001",
            "null",
            "",
            "boards.example.com",
            "file:///boards",
        ] {
            assert!(!origins.allows_origin(origin), "{origin}");
        }

        let plain = Origins::new("http://localhost:8000", &[]).expect("valid");
        assert_eq!(plain.https(), Https::No);
        assert!(plain.allows_origin("http://localhost:8000"));
        assert!(!plain.allows_origin("http://localhost:8001"));
        assert!(!plain.allows_origin("http://127.0.0.1:8000"));
    }

    #[test]
    fn the_origin_of_a_url_is_its_scheme_host_and_port_in_one_spelling() {
        for (url, origin) in [
            ("http://127.0.0.1:8000/boards", "http://127.0.0.1:8000"),
            ("http://127.0.0.1:80", "http://127.0.0.1"),
            ("http://[::1]:8000", "http://[::1]:8000"),
            ("https://[2001:DB8::1]:443/x", "https://[2001:db8::1]"),
            ("http://[::1]", "http://[::1]"),
            (
                "HTTPS://Boards.Example.COM:443",
                "https://boards.example.com",
            ),
            ("https://boards.example.com:", "https://boards.example.com"),
            ("http://localhost:8080?x=1#y", "http://localhost:8080"),
            (
                "https://someone:secret@example.com/x",
                "https://example.com",
            ),
            (" https://example.com ", "https://example.com"),
        ] {
            assert_eq!(origin_of(url).as_deref(), Some(origin), "{url}");
        }
        for url in [
            "",
            "null",
            "example.com",
            "localhost:8000",
            "ftp://example.com",
            "ws://example.com",
            "https://",
            "https://:443",
            "https://someone@",
            "http://[::1",
            "http://[zz::1]:80",
            "http://[::1]8000",
            "http://example.com:99999",
            "http://example.com:+80",
            "http://example.com:x",
            "http://exa mple.com",
            "/boards",
        ] {
            assert_eq!(origin_of(url), None, "{url:?}");
        }
    }

    #[test]
    fn what_is_no_http_origin_is_refused_at_start() {
        for server_url in [
            "",
            "localhost:8000",
            "ftp://example.com",
            "https://",
            "/boards",
        ] {
            assert!(Origins::new(server_url, &[]).is_err(), "{server_url:?}");
        }
        assert!(
            Origins::new("http://localhost:8000", &["*".to_owned()]).is_err(),
            "no wildcard among the allowed origins"
        );
    }
}
