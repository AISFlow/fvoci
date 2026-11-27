//! The `security_contact` setting: the `Contact` URI served at `/.well-known/security.txt`.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::bail;

/// A validated `Contact` URI for `/.well-known/security.txt` (RFC 9116, section 2.5.3).
///
/// Holds a single-line `mailto:`, `https://`, or `tel:` URI with no whitespace or
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecurityContact(String);

impl SecurityContact {
    /// Parses `security_contact`; an empty string returns `Ok(None)` so the endpoint
    /// stays disabled (404).
    ///
    /// # Errors
    ///
    /// Returns an error if `raw` is non-empty and contains whitespace or control
    /// characters, or is not a valid `mailto:`, `https://`, or `tel:` URI per RFC 9116.
    pub fn parse(raw: &str) -> anyhow::Result<Option<Self>> {
        if raw.is_empty() {
            return Ok(None);
        }
        if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
            bail!("security_contact must not contain whitespace or control characters");
        }
        let valid = if let Some(rest) = raw.strip_prefix("mailto:") {
            is_valid_mailto(rest)
        } else if let Some(rest) = raw.strip_prefix("https://") {
            is_valid_https(rest)
        } else if let Some(rest) = raw.strip_prefix("tel:") {
            is_valid_tel(rest)
        } else {
            false
        };
        if !valid {
            bail!(
                "security_contact {raw:?} must be a mailto:, https://, or tel: URI \
                 (RFC 9116), such as \"mailto:security@example.com\" or \
                 \"https://example.com/security\""
            );
        }
        Ok(Some(Self(raw.to_owned())))
    }

    /// The URI as written in the `Contact` field.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecurityContact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for SecurityContact {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Checks the part after `mailto:` per RFC 6068: `local@domain` with an optional
/// RFC 3986 `?query` and no `#fragment`.
fn is_valid_mailto(rest: &str) -> bool {
    if !rest.is_ascii() || rest.contains('#') {
        return false;
    }
    let (address, query) = rest
        .split_once('?')
        .map_or((rest, None), |(addr, q)| (addr, Some(q)));
    if !query.is_none_or(|q| is_valid_rfc3986_segment(q, UriSegment::QueryOrFragment)) {
        return false;
    }
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    is_valid_mailto_local(local) && domain.contains('.') && is_valid_dns_host(domain)
}

/// Checks the local part of a `mailto:` address: RFC 5322 `dot-atom` characters or
/// valid `%HH` escapes, with no leading, trailing, or consecutive dots.
fn is_valid_mailto_local(local: &str) -> bool {
    if local.is_empty() || local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    let bytes = local.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if !is_percent_encoded(bytes, i) {
                    return false;
                }
                i += 3;
            }
            b if b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'$'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'='
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                ) =>
            {
                i += 1;
            }
            _ => return false,
        }
    }
    true
}

/// Checks the part after `https://`: ASCII RFC 3986 URI with a valid authority (no
/// embedded userinfo) and valid path, query, and fragment segments.
fn is_valid_https(rest: &str) -> bool {
    if !rest.is_ascii() {
        return false;
    }
    let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    is_valid_https_authority(authority) && is_valid_uri_tail(tail)
}

fn is_valid_https_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') || authority.contains('\\') {
        return false;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((addr, after)) = bracketed.split_once(']') else {
            return false;
        };
        if addr.parse::<Ipv6Addr>().is_err() {
            return false;
        }
        let port = match after {
            "" => None,
            rest => match rest.strip_prefix(':') {
                Some(port) => Some(port),
                None => return false,
            },
        };
        return port.is_none_or(is_valid_port);
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if !port.is_none_or(is_valid_port) {
        return false;
    }
    host.parse::<Ipv4Addr>().is_ok() || is_valid_dns_host(host)
}

/// Checks a DNS host name: non-empty labels of ASCII letters, digits, and `-` (not at
/// label ends), with a final label that is not all digits (rejecting invalid IPv4
/// dotted quads such as `999.999.999.999`).
fn is_valid_dns_host(host: &str) -> bool {
    let last_is_not_all_digits = host
        .rsplit('.')
        .next()
        .is_some_and(|tld| !tld.is_empty() && !tld.bytes().all(|byte| byte.is_ascii_digit()));
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && last_is_not_all_digits
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn is_valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|value| value > 0)
}

/// Which RFC 3986 URI component is being validated (`?` is allowed in query and
/// fragment components, not in path segments).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UriSegment {
    Path,
    QueryOrFragment,
}

/// Checks the `[path][?query][#fragment]` tail of an `https://` URI per RFC 3986.
fn is_valid_uri_tail(tail: &str) -> bool {
    let (before_frag, frag) = tail
        .split_once('#')
        .map_or((tail, None), |(before, f)| (before, Some(f)));
    let (path, query) = before_frag
        .split_once('?')
        .map_or((before_frag, None), |(p, q)| (p, Some(q)));
    is_valid_rfc3986_segment(path, UriSegment::Path)
        && query.is_none_or(|q| is_valid_rfc3986_segment(q, UriSegment::QueryOrFragment))
        && frag.is_none_or(|f| is_valid_rfc3986_segment(f, UriSegment::QueryOrFragment))
}

fn is_valid_rfc3986_segment(segment: &str, kind: UriSegment) -> bool {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if !is_percent_encoded(bytes, i) {
                    return false;
                }
                i += 3;
            }
            b if b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                        | b'/'
                )
                || (kind == UriSegment::QueryOrFragment && b == b'?') =>
            {
                i += 1;
            }
            _ => return false,
        }
    }
    true
}

fn is_percent_encoded(bytes: &[u8], index: usize) -> bool {
    bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
        && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
}

/// Checks the part after `tel:` per RFC 3966: either a global number (`+` followed by
/// digits and visual separators) or a local number with a `;phone-context=` parameter,
/// and no unencoded `#` fragment delimiter.
fn is_valid_tel(rest: &str) -> bool {
    if rest.is_empty() || !rest.is_ascii() || rest.contains('#') {
        return false;
    }
    let mut parts = rest.split(';');
    let Some(number) = parts.next() else {
        return false;
    };
    let (is_global, digits_part) = match number.strip_prefix('+') {
        Some(global) => (true, global),
        None => (false, number),
    };
    let valid_number = digits_part.bytes().any(|byte| byte.is_ascii_digit())
        && digits_part.bytes().all(|byte| {
            byte.is_ascii_digit()
                || matches!(byte, b'-' | b'.' | b'(' | b')')
                || (!is_global && matches!(byte, b'*' | b'A'..=b'D' | b'a'..=b'd'))
        });
    if !valid_number {
        return false;
    }
    let mut has_phone_context = false;
    for param in parts {
        let Some((name, value)) = param.split_once('=') else {
            return false;
        };
        let valid_name = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        let valid_value = !value.is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'-' | b'.' | b'_' | b'~' | b'%' | b':')
            });
        if !valid_name || !valid_value {
            return false;
        }
        if name.eq_ignore_ascii_case("phone-context") {
            has_phone_context = true;
        }
    }
    is_global || has_phone_context
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_disables_security_contact() {
        assert_eq!(
            SecurityContact::parse("").expect("empty is valid"),
            None,
            "empty means unset"
        );
    }

    #[test]
    fn valid_rfc9116_uris_are_accepted() {
        for raw in [
            "mailto:security@example.com",
            "mailto:sec+report@example.org?subject=Security%20Report",
            "https://example.com/security",
            "https://example.org:8443/report?scope=sec#disclosure",
            "https://[2001:db8::1]:8443/security",
            "tel:+1-555-0100",
            "tel:+1-555-0100;ext=42",
            "tel:555-0100;phone-context=example.com",
        ] {
            let contact = SecurityContact::parse(raw)
                .unwrap_or_else(|error| panic!("{raw} should be valid: {error}"))
                .expect("non-empty gives Some");
            assert_eq!(contact.as_str(), raw, "{raw}");
            assert_eq!(contact.to_string(), raw, "Display matches {raw}");
            assert_eq!(contact.as_ref(), raw, "AsRef<str> matches {raw}");
        }
    }

    #[test]
    fn invalid_or_unsafe_values_are_refused() {
        for raw in [
            " ",
            "security@example.com",
            "http://example.com/security",
            "ftp://example.com/security",
            "mailto:",
            "mailto:security",
            "mailto:@example.com",
            "mailto:security@",
            "mailto:security@localhost",
            "mailto:security@example..com",
            "mailto:security@-bad-.com",
            "mailto:security@example.com#evil",
            "mailto:security@example.com/path",
            "mailto:.security@example.com",
            "mailto:a\"b\\c@example.com",
            "mailto:security@example.com?subject=<x>",
            "mailto:security@example.com?body=%ZZ",
            "mailto:a@b@example.com",
            "mailto:<security@example.com>",
            "https://",
            "https:///security",
            "https://999.999.999.999/security",
            "https://example.com:0/security",
            "https://example.com:99999/security",
            "https://user:pass@example.com/security",
            "https://[::1/security",
            "https://example.com/<script>",
            "https://[2001:db8::1]/%ZZ",
            "https://example.com/path#a#b",
            "tel:",
            "tel:abc",
            "tel:abc1",
            "tel:555-0100",
            "tel:+1-555-0100#evil",
            "mailto:security@example.com\nExpires: 2099-01-01T00:00:00Z",
            "mailto:security@example.com\r\nCanonical: https://evil.example/",
            "https://example.com/security report",
        ] {
            let error = SecurityContact::parse(raw)
                .expect_err(&format!("{raw:?} must be refused"))
                .to_string();
            assert!(
                error.contains("security_contact"),
                "error names the key for {raw:?}: {error}"
            );
        }
    }
}
