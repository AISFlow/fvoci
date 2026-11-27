//! The reverse proxies whose word on a request's client is believed: `trusted_proxies`.
//!
//! A proxy puts the address it got the request from at the end of `X-Forwarded-For`.
//! Anybody can send that header, so only what a trusted proxy added to it counts: the
//! list is read from the right, past the proxies the configuration names, and the first
//! address that is no trusted proxy is the client. The left end is whatever the client
//! wrote there. The `Forwarded` header of RFC 7239 is not read. Reading the header off a
//! request is HTTP's (`crate::client_address`); what it says is read here.

use std::net::{IpAddr, SocketAddr};

/// A range of addresses: an address and how many of its leading bits count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AddressRange {
    address: IpAddr,
    prefix_bits: u32,
}

impl AddressRange {
    /// Reads `address` or `address/bits`.
    fn parse(text: &str) -> Option<Self> {
        let (address, bits) = match text.split_once('/') {
            Some((address, bits)) => (address, Some(bits)),
            None => (text, None),
        };
        let address: IpAddr = address.trim().parse().ok()?;
        let width = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix_bits: u32 = match bits {
            Some(bits) => bits.trim().parse().ok().filter(|bits| *bits <= width)?,
            None => width,
        };
        // The addresses of requests are compared in their canonical form, in which an
        // IPv4 address written as IPv6 (`::ffff:10.0.0.1`) is the IPv4 address. A range
        // written that way is the IPv4 range it means; one that covers more than the
        // mapped addresses means nothing that could be compared.
        match (address, address.to_canonical()) {
            (IpAddr::V6(_), mapped @ IpAddr::V4(_)) => Some(Self {
                address: mapped,
                prefix_bits: prefix_bits.checked_sub(96)?,
            }),
            _ => Some(Self {
                address,
                prefix_bits,
            }),
        }
    }

    fn contains(&self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(range), IpAddr::V4(address)) => same_leading_bits(
                u128::from(u32::from(range)),
                u128::from(u32::from(address)),
                32,
                self.prefix_bits,
            ),
            (IpAddr::V6(range), IpAddr::V6(address)) => same_leading_bits(
                u128::from(range),
                u128::from(address),
                128,
                self.prefix_bits,
            ),
            _ => false,
        }
    }
}

/// Whether two addresses of `width` bits agree in their first `bits` bits.
fn same_leading_bits(left: u128, right: u128, width: u32, bits: u32) -> bool {
    let ignored = width.saturating_sub(bits);
    // Shifting by the whole width is not defined; nothing counts then.
    ignored >= width || (left >> ignored) == (right >> ignored)
}

/// The reverse proxies whose `X-Forwarded-For` is believed: `trusted_proxies`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrustedProxies(Vec<AddressRange>);

impl TrustedProxies {
    /// Reads a list of addresses and address ranges (`10.0.0.0/8`, `::1`).
    ///
    /// # Errors
    ///
    /// Returns the first entry that is neither.
    pub fn parse(ranges: &[String]) -> Result<Self, String> {
        ranges
            .iter()
            .map(|range| {
                AddressRange::parse(range).ok_or_else(|| {
                    format!("trusted_proxies has {range:?}, which is no address or address range")
                })
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn trusts(&self, address: IpAddr) -> bool {
        self.0.iter().any(|range| range.contains(address))
    }

    /// The address of the client of a request that arrived from `peer`, the lines of its
    /// `X-Forwarded-For` being `forwarded`, in their order.
    ///
    /// That is `peer` unless `peer` is a trusted proxy. Then it is the last address of
    /// those lines that is no trusted proxy; if every address there is one, the first of
    /// them; and `peer` if there is none. An entry that is no address ends the reading:
    /// what stands to its left was written by whoever sent it. An entry may carry a port
    /// (`203.0.113.7:51234`, `[2001:db8::1]:443`), as some proxies write it.
    pub fn client_of<'line>(
        &self,
        peer: IpAddr,
        forwarded: impl IntoIterator<Item = &'line [u8]>,
    ) -> IpAddr {
        let peer = peer.to_canonical();
        if !self.trusts(peer) {
            return peer;
        }
        // By bytes: a line with a byte that is no text still has entries to the right of
        // that byte, and those were written by the proxies.
        let forwarded: Vec<&[u8]> = forwarded
            .into_iter()
            .flat_map(|line| line.split(|byte| *byte == b','))
            .collect();
        let mut client = peer;
        // From the right: each trusted proxy vouches for the address before it.
        for entry in forwarded.into_iter().rev() {
            let Some(address) = forwarded_address(entry) else {
                break;
            };
            client = address.to_canonical();
            if !self.trusts(client) {
                break;
            }
        }
        client
    }
}

/// The address of one entry of `X-Forwarded-For`: an address, or an address and a port.
fn forwarded_address(entry: &[u8]) -> Option<IpAddr> {
    let entry = std::str::from_utf8(entry).ok()?.trim();
    entry
        .parse::<IpAddr>()
        .or_else(|_| entry.parse::<SocketAddr>().map(|socket| socket.ip()))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxies(ranges: &[&str]) -> TrustedProxies {
        let ranges: Vec<String> = ranges.iter().map(|range| (*range).to_owned()).collect();
        TrustedProxies::parse(&ranges).expect("valid ranges")
    }

    fn client(proxies: &TrustedProxies, peer: &str, forwarded: &[&'static str]) -> String {
        proxies
            .client_of(
                peer.parse().expect("an address"),
                forwarded.iter().map(|line| line.as_bytes()),
            )
            .to_string()
    }

    #[test]
    fn without_trusted_proxies_the_peer_is_the_client_whatever_the_header_says() {
        let nobody = proxies(&[]);
        assert_eq!(client(&nobody, "203.0.113.7", &[]), "203.0.113.7");
        assert_eq!(
            client(&nobody, "203.0.113.7", &["198.51.100.1, 10.0.0.1"]),
            "203.0.113.7",
            "a header from somebody who is no proxy is not believed"
        );
    }

    #[test]
    fn behind_a_trusted_proxy_the_client_is_what_the_proxy_added() {
        let one = proxies(&["10.0.0.0/8"]);
        assert_eq!(
            client(&one, "10.1.2.3", &["203.0.113.7"]),
            "203.0.113.7",
            "the address the proxy saw"
        );
        // The client wrote the left part itself; the proxy appended what it saw.
        assert_eq!(
            client(&one, "10.1.2.3", &["1.1.1.1, 203.0.113.7"]),
            "203.0.113.7",
            "not what the client claims to be"
        );
        assert_eq!(
            client(&one, "10.1.2.3", &["1.1.1.1", "203.0.113.7"]),
            "203.0.113.7",
            "several header lines are one list"
        );
        assert_eq!(
            client(&one, "203.0.113.9", &["198.51.100.1"]),
            "203.0.113.9",
            "a request that did not come through the proxy"
        );
        assert_eq!(client(&one, "10.1.2.3", &[]), "10.1.2.3", "no header");
    }

    #[test]
    fn several_trusted_proxies_are_passed_from_the_right() {
        let two = proxies(&["10.0.0.0/8", "192.168.1.1"]);
        assert_eq!(
            client(&two, "10.0.0.1", &["9.9.9.9, 203.0.113.7, 192.168.1.1"]),
            "203.0.113.7"
        );
        assert_eq!(
            client(&two, "10.0.0.1", &["10.0.0.5, 192.168.1.1"]),
            "10.0.0.5",
            "all trusted: the first of them"
        );
        assert_eq!(
            client(
                &two,
                "10.0.0.1",
                &["203.0.113.7, not an address, 192.168.1.1"]
            ),
            "192.168.1.1",
            "what stands left of something that is no address is not read"
        );
    }

    #[test]
    fn ranges_of_both_families_and_mapped_addresses() {
        let both = proxies(&["::1", "fd00::/8", "127.0.0.0/8", "0.0.0.0/0"]);
        assert_eq!(client(&both, "::1", &["2001:db8::7"]), "2001:db8::7");
        assert_eq!(client(&both, "fd12::1", &["2001:db8::7"]), "2001:db8::7");
        let v4 = proxies(&["127.0.0.1"]);
        assert_eq!(
            client(&v4, "::ffff:127.0.0.1", &["203.0.113.7"]),
            "203.0.113.7",
            "an IPv4 peer on a dual-stack socket"
        );
        assert_eq!(client(&v4, "127.0.0.2", &["203.0.113.7"]), "127.0.0.2");
        let all_v6 = proxies(&["::/0"]);
        assert_eq!(
            client(&all_v6, "2001:db8::1", &["203.0.113.7"]),
            "203.0.113.7"
        );
        assert_eq!(
            client(&all_v6, "198.51.100.1", &["203.0.113.7"]),
            "198.51.100.1",
            "a range of one family holds no address of the other"
        );
    }

    #[test]
    fn a_range_of_mapped_addresses_is_the_ipv4_range_it_means() {
        let mapped = proxies(&["::ffff:10.0.0.0/104", "::ffff:192.0.2.1"]);
        for peer in ["10.1.2.3", "::ffff:10.1.2.3", "192.0.2.1"] {
            assert_eq!(
                client(&mapped, peer, &["203.0.113.7"]),
                "203.0.113.7",
                "{peer}"
            );
        }
        assert_eq!(client(&mapped, "11.0.0.1", &["203.0.113.7"]), "11.0.0.1");
        assert_eq!(client(&mapped, "192.0.2.2", &["203.0.113.7"]), "192.0.2.2");
    }

    #[test]
    fn an_entry_may_carry_a_port() {
        let trusted = proxies(&["127.0.0.1"]);
        assert_eq!(
            client(&trusted, "127.0.0.1", &["203.0.113.7:51234"]),
            "203.0.113.7"
        );
        assert_eq!(
            client(&trusted, "127.0.0.1", &["[2001:db8::1]:443"]),
            "2001:db8::1"
        );
    }

    #[test]
    fn a_byte_that_is_no_text_ends_the_reading_where_it_stands() {
        // What the client wrote is unreadable; what the proxy added to the right of it
        // on the same line is still the client's address.
        let trusted = proxies(&["127.0.0.1"]);
        let line: &[u8] = b"\xff, 203.0.113.7";
        assert_eq!(
            trusted
                .client_of("127.0.0.1".parse().expect("an address"), [line])
                .to_string(),
            "203.0.113.7"
        );
    }

    #[test]
    fn what_is_no_range_is_refused() {
        for range in [
            "",
            "localhost",
            "10.0.0.0/33",
            "::1/129",
            "10.0.0.0/x",
            "10.0.0.0/-1",
            // Wider than the mapped addresses: it has no IPv4 range to mean.
            "::ffff:10.0.0.0/64",
        ] {
            assert!(
                TrustedProxies::parse(&[range.to_owned()]).is_err(),
                "{range:?}"
            );
        }
    }
}
