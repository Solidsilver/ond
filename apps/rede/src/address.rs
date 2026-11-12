//! A server's address as it's typed: with a scheme or without one. A host on
//! this computer or this network usually has no certificate at all, so a bare
//! one is tried over https and then over plain http; a host on the internet
//! is only ever tried over https, since a plain-http sign-in there would put
//! the password on the wire.

use std::net::IpAddr;

/// The addresses to try for `typed`, in order. A typed scheme is used as it
/// is; a bare host gets https first, and http after it only when the host is
/// on this network.
pub fn candidates(typed: &str) -> Vec<String> {
    let typed = typed.trim();
    if typed.contains("://") {
        return vec![typed.to_owned()];
    }
    let mut addresses = vec![format!("https://{typed}")];
    if is_local(typed) {
        addresses.push(format!("http://{typed}"));
    }
    addresses
}

/// Whether `typed` names this computer or a machine on this network: the
/// addresses where a server is likely to be plain http.
pub fn is_local(typed: &str) -> bool {
    let host = host(typed).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
        Err(_) => false,
    }
}

/// The host part of a typed address: no scheme, path, port or brackets.
fn host(typed: &str) -> &str {
    let rest = typed.split_once("://").map_or(typed, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or(bracketed);
    }
    // A port is digits after the last colon; a bare IPv6 address has more
    // than one colon and so keeps all of itself.
    match authority.rsplit_once(':') {
        Some((before, after)) if !before.contains(':') && after.chars().all(|c| c.is_ascii_digit()) => before,
        _ => authority,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_on_the_internet_is_only_tried_over_https() {
        assert_eq!(candidates("books.example.com"), ["https://books.example.com"]);
        assert_eq!(candidates("books.example.com:13378"), ["https://books.example.com:13378"]);
        assert!(!is_local("books.example.com:13378"));
        assert!(!is_local("172.15.0.1"), "just outside the private range");
        assert!(!is_local("172.32.0.1"), "and just past its end");
        assert!(!is_local("books.example.local.example.com"), "a name that only ends in a local part of its own");
        assert_eq!(candidates("nas.local.example.com"), ["https://nas.local.example.com"]);
        assert!(!is_local("2001:db8::1"));
    }

    #[test]
    fn each_private_range_and_localhost_is_tried_over_http_too() {
        for host in [
            "localhost",
            "localhost:13378",
            "127.0.0.1",
            "127.1.2.3:8080",
            "10.0.0.5",
            "10.1.2.3:13378",
            "172.16.0.1",
            "172.31.255.254:80",
            "192.168.1.5",
            "192.168.1.5:13378",
            "169.254.10.2",
            "nas.local",
            "NAS.local:13378",
            "[::1]",
            "[::1]:13378",
            "::1",
            "[fe80::1]",
            "[fe80::1]:8080",
            "[fd00::1]:13378",
        ] {
            assert!(is_local(host), "{host}");
            let addresses = candidates(host);
            assert_eq!(addresses.len(), 2, "{host}: {addresses:?}");
            assert_eq!(addresses[0], format!("https://{host}"));
            assert_eq!(addresses[1], format!("http://{host}"));
        }
    }

    #[test]
    fn an_address_with_a_scheme_is_used_as_it_is_typed() {
        assert_eq!(candidates("https://books.example.com"), ["https://books.example.com"]);
        assert_eq!(candidates(" http://192.168.1.5:13378 "), ["http://192.168.1.5:13378"]);
        assert_eq!(candidates("https://nas.local:13378/"), ["https://nas.local:13378/"], "even a local one: the scheme is the user's");
        assert_eq!(candidates("HTTP://BOOKS.EXAMPLE.COM"), ["HTTP://BOOKS.EXAMPLE.COM"]);
    }

    #[test]
    fn the_host_is_read_without_its_scheme_port_or_brackets() {
        assert_eq!(host("https://NAS.local:13378/books?x=1"), "NAS.local");
        assert_eq!(host("192.168.1.5:13378"), "192.168.1.5");
        assert_eq!(host("[::1]:13378"), "::1");
        assert_eq!(host("::1"), "::1");
        assert_eq!(host("books.example.com"), "books.example.com");
    }
}
