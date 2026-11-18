//! The `X-Forwarded-For` resolution: trusted entries, joined header lists,
//! the rightmost-untrusted rule.

use std::net::{IpAddr, Ipv4Addr};

/// Resolve the client address from the input as one request would: the
/// first line is the transport peer (when it parses as an address, else
/// `127.0.0.1`), every following line that parses as a trusted entry is one,
/// the rest are `X-Forwarded-For` header values in wire order.
///
/// The differentials: with no trusted proxy at all the answer must be the
/// peer whatever the headers claim, and an answer that is not the peer can
/// only be one of the hops the headers actually list.
///
/// # Panics
///
/// When a resolution contradicts one of those, or the parsers panic.
pub fn proxy(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.split('\n');
    let peer = lines
        .next()
        .map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |line| {
            line.trim()
                .parse::<IpAddr>()
                .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
        });
    let mut trusted: Vec<String> = Vec::new();
    let mut headers: Vec<(String, Vec<u8>)> = Vec::new();
    for line in lines {
        let line = line.trim_matches(['\r', ' ', '\t']);
        if line.is_empty() {
            continue;
        }
        if timbre_edge_http::fuzzing::trusted_entry_accepted(line).is_ok() {
            trusted.push(line.to_owned());
        } else {
            headers.push(("x-forwarded-for".to_owned(), line.as_bytes().to_vec()));
        }
    }
    // Nothing is trusted: the header is client noise and the peer stands.
    let alone = timbre_edge_http::fuzzing::client_address_from(peer, &headers, &[])
        .expect("an empty trusted list cannot fail to parse");
    assert_eq!(
        alone, peer,
        "an untrusted peer keeps its own address whatever the header says"
    );
    let Ok(resolved) = timbre_edge_http::fuzzing::client_address_from(peer, &headers, &trusted)
    else {
        return;
    };
    if resolved != peer {
        // The answer can only be one of the hops the headers list: split
        // them the way the resolver does and demand membership. A hop
        // spelled differently from every header fragment - or invented - is
        // the bug this differential catches.
        let listed: Vec<IpAddr> = headers
            .iter()
            .flat_map(|(_, value)| {
                String::from_utf8_lossy(value)
                    .split(',')
                    .map(|hop| hop.trim().to_owned())
                    .collect::<Vec<String>>()
            })
            .filter_map(|hop| hop.parse::<IpAddr>().ok())
            .collect();
        assert!(
            listed.contains(&resolved),
            "the client address must be one of the listed hops: {resolved} not among {listed:?}"
        );
        // A hop that is itself fully trusted cannot end the scan as the
        // client: the rule returns the first untrusted one.
        for entry in &trusted {
            if let Ok(network) = entry
                .split_once('/')
                .map_or(entry.as_str(), |(network, _)| network)
                .parse::<IpAddr>()
            {
                let whole = u32::from(network.is_ipv6()) * 128 + u32::from(network.is_ipv4()) * 32;
                let bits = entry
                    .split_once('/')
                    .and_then(|(_, prefix)| prefix.parse::<u8>().ok())
                    .unwrap_or(u8::try_from(whole).unwrap_or(u8::MAX));
                assert!(
                    !(bits == u8::try_from(whole).unwrap_or(u8::MAX) && network == resolved),
                    "a fully trusted address cannot be the client: {resolved} is listed as {entry}"
                );
            }
        }
    }
}
