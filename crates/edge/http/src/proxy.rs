//! The client address behind proxies: trust comes from configuration, never
//! from a header alone.

use crate::failures::HttpFailure;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use timbre_kernel::Fallible;

/// One trusted proxy: an address plus its prefix length, i.e. what
/// `trusted_proxies` lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Trusted {
    network: IpAddr,
    bits: u8,
}

impl Trusted {
    /// Parse a trusted entry: `10.0.0.5`, `10.0.0.0/8`, `::1` or `fd00::/8`.
    ///
    /// # Errors
    ///
    /// `http.proxy_invalid` for text that is neither an address nor an
    /// address with a valid prefix length.
    pub(crate) fn parse(entry: &str) -> Fallible<Trusted> {
        let (address, bits) = if let Some((address, prefix)) = entry.split_once('/') {
            let bits: u8 = prefix.parse().map_err(|_| invalid(entry))?;
            let address = address.parse::<IpAddr>().map_err(|_| invalid(entry))?;
            let width = if address.is_ipv4() { 32u8 } else { 128 };
            if bits > width {
                return Err(invalid(entry));
            }
            (address, bits)
        } else {
            let address = entry.parse::<IpAddr>().map_err(|_| invalid(entry))?;
            // A bare address is a /32 (or /128) prefix.
            let width = if address.is_ipv4() { 32u8 } else { 128 };
            (address, width)
        };
        Ok(Trusted {
            network: address,
            bits,
        })
    }

    /// Whether `address` is inside this trusted network.
    #[must_use]
    fn contains(self, address: IpAddr) -> bool {
        let (left, right) = match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(probe)) => (
                u128::from(Octets::from(network)),
                u128::from(Octets::from(probe)),
            ),
            (IpAddr::V6(network), IpAddr::V6(probe)) => {
                (Octets::from(network).into(), Octets::from(probe).into())
            }
            // A v4 peer is never inside a v6 prefix: `::ffff:127.0.0.1` must
            // be listed as an IPv6 prefix to be trusted.
            _ => return false,
        };
        let shift = 128 - u32::from(self.bits);
        (left >> shift) == (right >> shift)
    }
}

fn invalid(entry: &str) -> Box<dyn timbre_kernel::Failure> {
    Box::new(HttpFailure::ProxyInvalid {
        entry: entry.to_owned(),
    })
}

/// A 16-byte view of any address so v4 and v6 compare the same way.
struct Octets([u8; 16]);

impl From<Ipv4Addr> for Octets {
    fn from(address: Ipv4Addr) -> Self {
        let bytes = address.octets();
        Octets([
            bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
    }
}

impl From<Ipv6Addr> for Octets {
    fn from(address: Ipv6Addr) -> Self {
        Octets(address.octets())
    }
}

impl From<Octets> for u128 {
    fn from(octets: Octets) -> Self {
        u128::from_be_bytes(octets.0)
    }
}

/// The client address of a request: `peer` itself unless it is trusted and
/// `forwarded` (the raw `X-Forwarded-For` header) names an untrusted hop.
///
/// The rightmost address of the list that is not itself trusted wins: the
/// left part of the header is client-controlled, so only the entry this
/// trusted proxy appended is dependable.
#[must_use]
pub(crate) fn client_address(peer: IpAddr, forwarded: Option<&str>, trusted: &[Trusted]) -> IpAddr {
    if !trusted.iter().any(|proxy| proxy.contains(peer)) {
        return peer;
    }
    let Some(list) = forwarded else {
        return peer;
    };
    let hops: Vec<&str> = list
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    for hop in hops.iter().rev() {
        if let Ok(address) = hop.parse::<IpAddr>()
            && !trusted.iter().any(|proxy| proxy.contains(address))
        {
            return address;
        }
    }
    // Every hop was a trusted proxy: the peer is the only real address left.
    peer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust(entries: &[&str]) -> Vec<Trusted> {
        entries
            .iter()
            .map(|entry| Trusted::parse(entry).expect("a valid trusted entry"))
            .collect()
    }

    #[test]
    fn an_untrusted_peer_keeps_its_own_address() {
        let trusted = trust(&["10.0.0.0/8"]);
        let peer = "203.0.113.7".parse().expect("an address");
        assert_eq!(
            client_address(peer, Some("198.51.100.9"), &trusted),
            peer,
            "a header from an untrusted peer is client noise"
        );
    }

    #[test]
    fn the_rightmost_untrusted_hop_wins() {
        let trusted = trust(&["10.0.0.1"]);
        let peer = "10.0.0.1".parse().expect("an address");
        assert_eq!(
            client_address(peer, Some("198.51.100.1, 203.0.113.9, 10.0.0.1"), &trusted),
            "203.0.113.9"
                .parse::<std::net::IpAddr>()
                .expect("an address"),
            "the left part is forged; this proxy appended the right end"
        );
    }

    #[test]
    fn all_trusted_hops_fall_back_to_the_peer() {
        let trusted = trust(&["10.0.0.0/8"]);
        let peer = "10.0.0.1".parse().expect("an address");
        assert_eq!(
            client_address(peer, Some("10.0.0.2, 10.0.0.3"), &trusted),
            peer
        );
    }

    #[test]
    fn a_prefix_matches_inside_and_not_outside() {
        let trusted = trust(&["10.0.0.0/8", "fd00::/8"]);
        assert!(trusted[0].contains("10.255.0.1".parse().expect("an address")));
        assert!(!trusted[0].contains("11.0.0.1".parse().expect("an address")));
        assert!(trusted[1].contains("fd12::5".parse().expect("an address")));
        assert!(!trusted[1].contains("fe12::5".parse().expect("an address")));
    }

    #[test]
    fn a_malformed_trusted_entry_is_rejected() {
        for bad in ["not-an-address", "10.0.0.0/33", "10.0.0.0/x", ""] {
            let error = Trusted::parse(bad).expect_err("malformed");
            assert_eq!(error.code(), crate::failures::PROXY_INVALID, "for {bad:?}");
        }
    }
}
