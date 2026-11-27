//! Who a request comes from: the address of the client, also behind reverse proxies
//! (`crate::config::TrustedProxies` says which proxies are believed, and how).
//!
//! The request log and the limits on login attempts use the same answer, so that what
//! is limited is what the log shows.

use std::net::IpAddr;

use axum::http::{HeaderMap, HeaderValue};

use crate::config::TrustedProxies;

/// The header a reverse proxy lists the client's address in.
const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// The address of the client of a request, where it has one: a request through the
/// administration socket has none. Put into the request's extensions by the request
/// log, for the handlers that count by address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddress(pub Option<IpAddr>);

/// The address of the client of a request that arrived from `peer` with `headers`, by
/// what `proxies` add to its `X-Forwarded-For` ([`TrustedProxies::client_of`]).
pub fn client_of(proxies: &TrustedProxies, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    proxies.client_of(
        peer,
        headers
            .get_all(X_FORWARDED_FOR)
            .iter()
            .map(HeaderValue::as_bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_of_the_header_are_one_list_read_by_bytes() {
        let ranges = ["127.0.0.1".to_owned()];
        let trusted = TrustedProxies::parse(&ranges).expect("a valid range");
        let mut headers = HeaderMap::new();
        headers.append(X_FORWARDED_FOR, HeaderValue::from_static("198.51.100.1"));
        headers.append(
            X_FORWARDED_FOR,
            HeaderValue::from_bytes(b"\xff, 203.0.113.7").expect("a header value"),
        );
        let peer: IpAddr = "127.0.0.1".parse().expect("an address");
        assert_eq!(
            client_of(&trusted, peer, &headers).to_string(),
            "203.0.113.7"
        );
        assert_eq!(
            client_of(&trusted, peer, &HeaderMap::new()),
            peer,
            "no header"
        );
    }
}
