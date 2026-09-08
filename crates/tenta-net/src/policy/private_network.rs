//! Private-network and destination policy (`docs/spec/03-network.md` §8,
//! `docs/spec/19-security.md` §2).
//!
//! A headless engine driven by an agent is a confused deputy waiting to
//! happen: a page it is told to open can point at `169.254.169.254` or at a
//! service on loopback. Every destination is therefore classified before the
//! connection and re-classified after every DNS answer and every redirect.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use parking_lot::RwLock;
use tenta_core::{Error, Result};
use url::{Host, Url};

use super::error_names::ErrorName;

/// What kind of address a destination resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum AddressClass {
    /// `0.0.0.0` / `::`, which routes to loopback on most stacks.
    Unspecified,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918 `10/8`, `172.16/12`, `192.168/16`; IPv6 ULA `fc00::/7`.
    Private,
    /// `169.254.0.0/16`, `fe80::/10`. Cloud metadata lives here.
    LinkLocal,
    /// RFC 6598 shared address space `100.64.0.0/10`.
    CarrierGradeNat,
    /// `224.0.0.0/4`, `ff00::/8`.
    Multicast,
    /// `192.0.2/24`, `198.51.100/24`, `203.0.113/24`, `2001:db8::/32`.
    Documentation,
    /// `198.18.0.0/15`, `2001:2::/48`.
    Benchmarking,
    /// `192.0.0.0/24`, `240.0.0.0/4`, `100::/64`, `2001::/23`.
    Reserved,
    /// `255.255.255.255`.
    Broadcast,
    /// Anything routable on the public internet.
    Public,
}

impl AddressClass {
    /// Whether the address is a normal public unicast destination.
    pub fn is_public(self) -> bool {
        self == AddressClass::Public
    }

    /// Whether reaching this address means reaching the host's own network.
    ///
    /// Everything that is not public unicast is treated as local: a fetch to
    /// a multicast, broadcast or reserved address is never something a page
    /// should be able to cause either.
    pub fn is_local(self) -> bool {
        !self.is_public()
    }
}

/// Classifies an address for the private-network check.
pub fn classify(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            // An IPv4-mapped or IPv4-compatible address reaches the embedded
            // IPv4 destination, so it must be classified as that address.
            Some(v4) => classify_v4(v4),
            None => match ipv4_compatible(v6).or_else(|| nat64_embedded(v6)) {
                Some(v4) => classify_v4(v4),
                None => classify_v6(v6),
            },
        },
    }
}

/// The IPv4 address behind the well-known NAT64 prefix `64:ff9b::/96`
/// (RFC 6052 §2.1).
///
/// On an IPv6-only network a NAT64 gateway forwards `64:ff9b::7f00:1` to
/// `127.0.0.1`, so the embedded address is the one that decides whether the
/// destination is private. The local-use prefix `64:ff9b:1::/48` (RFC 8215)
/// has no fixed embedding format and is treated as private wholesale.
fn nat64_embedded(ip: Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let segments = ip.segments();
    if segments[0] != 0x0064 || segments[1] != 0xff9b {
        return None;
    }
    if segments[2..6].iter().any(|segment| *segment != 0) {
        return None;
    }
    let octets = ip.octets();
    Some(std::net::Ipv4Addr::new(
        octets[12], octets[13], octets[14], octets[15],
    ))
}

/// `::a.b.c.d`, deprecated but still routed to the embedded IPv4 address by
/// some stacks. `::` and `::1` are excluded: they have their own meaning.
fn ipv4_compatible(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let octets = v6.octets();
    if octets[..12].iter().any(|b| *b != 0) {
        return None;
    }
    let v4 = Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
    if v4.octets()[0] == 0 { None } else { Some(v4) }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    let [a, b, c, _] = ip.octets();
    if ip.is_unspecified() {
        return AddressClass::Unspecified;
    }
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    if ip.is_broadcast() {
        return AddressClass::Broadcast;
    }
    if ip.is_multicast() {
        return AddressClass::Multicast;
    }
    if ip.is_link_local() {
        return AddressClass::LinkLocal;
    }
    if ip.is_private() {
        return AddressClass::Private;
    }
    if a == 100 && (64..128).contains(&b) {
        return AddressClass::CarrierGradeNat;
    }
    if ip.is_documentation() {
        return AddressClass::Documentation;
    }
    if a == 198 && (b == 18 || b == 19) {
        return AddressClass::Benchmarking;
    }
    if a == 192 && b == 0 && c == 0 {
        return AddressClass::Reserved;
    }
    if a == 0 || a >= 240 {
        return AddressClass::Reserved;
    }
    AddressClass::Public
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    let segments = ip.segments();
    if ip.is_unspecified() {
        return AddressClass::Unspecified;
    }
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    if ip.is_multicast() {
        return AddressClass::Multicast;
    }
    if segments[0] & 0xfe00 == 0xfc00 {
        return AddressClass::Private;
    }
    if segments[0] & 0xffc0 == 0xfe80 {
        return AddressClass::LinkLocal;
    }
    if segments[0] & 0xffc0 == 0xfec0 {
        // Deprecated site-local unicast (RFC 3879); still local if routed.
        return AddressClass::Private;
    }
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return AddressClass::Documentation;
    }
    if segments[0] == 0x3fff && segments[1] & 0xf000 == 0x0000 {
        // RFC 9637 documentation range 3fff::/20.
        return AddressClass::Documentation;
    }
    if segments[0] == 0x2001 && segments[1] == 0x0002 {
        return AddressClass::Benchmarking;
    }
    if segments[0] == 0x0064 && segments[1] == 0xff9b {
        // The local-use NAT64 prefix 64:ff9b:1::/48 (RFC 8215): a translator
        // on this network, so the destination is behind it.
        return AddressClass::Private;
    }
    if segments[0] == 0x0100 && segments[1..4].iter().all(|s| *s == 0) {
        // RFC 6666 discard prefix 100::/64.
        return AddressClass::Reserved;
    }
    if segments[0] == 0x2001 && segments[1] & 0xfe00 == 0x0000 {
        // IETF protocol assignments 2001::/23 (Teredo, ORCHIDv2, …).
        return AddressClass::Reserved;
    }
    AddressClass::Public
}

/// Whether a host name reaches the local network without a DNS answer.
///
/// `localhost` and `.local` never leave the machine or the link; `.localhost`
/// is reserved by RFC 6761 for loopback and Chromium resolves it that way.
pub fn is_private_host(host: &str) -> bool {
    let host = host
        .trim_end_matches('.')
        .trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return classify(ip).is_local();
    }
    let lower = host.to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || lower == "local"
        || lower.ends_with(".local")
}

/// Chromium's restricted port list (`net/base/port_util.cc`): ports whose
/// protocols can be driven into a request smuggling or cross-protocol attack
/// by an attacker who controls a URL.
pub const RESTRICTED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6679, 6697, 10080,
];

/// The destination rules of one BrowserContext.
#[derive(Debug, Clone)]
pub struct DestinationPolicy {
    /// Test harnesses (WPT, fixtures, bench) set this; production contexts
    /// do not (`docs/spec/03-network.md` §8).
    pub allow_private_network: bool,
    pub allowed_schemes: BTreeSet<String>,
    pub blocked_ports: BTreeSet<u16>,
    pub max_redirects: usize,
}

impl Default for DestinationPolicy {
    fn default() -> Self {
        Self {
            allow_private_network: false,
            allowed_schemes: ["http", "https", "ws", "wss"]
                .into_iter()
                .map(String::from)
                .collect(),
            blocked_ports: RESTRICTED_PORTS.iter().copied().collect(),
            max_redirects: 20,
        }
    }
}

impl DestinationPolicy {
    pub fn permissive() -> Self {
        Self {
            allow_private_network: true,
            ..Self::default()
        }
    }

    pub fn allow_private_network(mut self, allow: bool) -> Self {
        self.allow_private_network = allow;
        self
    }

    pub fn allow_scheme(mut self, scheme: &str) -> Self {
        self.allowed_schemes.insert(scheme.to_ascii_lowercase());
        self
    }

    /// Checks scheme, port and host name. Callable before DNS.
    pub fn check_url(&self, url: &Url) -> Result<()> {
        if !self.allowed_schemes.contains(url.scheme()) {
            return Err(denied(
                ErrorName::BlockedByClient,
                format!("scheme {} is not allowed", url.scheme()),
            ));
        }
        if let Some(port) = url.port()
            && self.blocked_ports.contains(&port)
        {
            return Err(denied(
                ErrorName::UnsafePort,
                format!("port {port} is on the restricted port list"),
            ));
        }
        if !self.allow_private_network
            && let Some(host) = url.host_str()
            && is_private_host(host)
        {
            return Err(denied(
                ErrorName::AccessDenied,
                format!("{host} is a private-network destination"),
            ));
        }
        Ok(())
    }

    /// Checks the URL and the addresses it resolved to.
    pub fn check(&self, url: &Url, resolved: &[IpAddr]) -> Result<()> {
        self.check_url(url)?;
        if self.allow_private_network {
            return Ok(());
        }
        for ip in resolved {
            let class = classify(*ip);
            if class.is_local() {
                return Err(denied(
                    ErrorName::AccessDenied,
                    format!("{ip} is {class:?}, a private-network destination"),
                ));
            }
        }
        Ok(())
    }

    /// Checks a redirect hop count. `hop` is the number of redirects already
    /// followed, so the first redirect is hop 1.
    pub fn check_redirect(&self, hop: usize) -> Result<()> {
        if hop > self.max_redirects {
            return Err(denied(
                ErrorName::TooManyRedirects,
                format!(
                    "{hop} redirects exceeds the limit of {}",
                    self.max_redirects
                ),
            ));
        }
        Ok(())
    }
}

fn denied(name: ErrorName, detail: String) -> Error {
    Error::NetDenied {
        reason: format!("{}: {detail}", name.as_str()),
    }
}

/// Guards against DNS rebinding: a name that resolved publicly on the first
/// lookup and privately on a later one is refused even if each individual
/// answer would pass on its own for a different name.
#[derive(Debug, Default)]
pub struct RebindGuard {
    seen: RwLock<HashMap<String, BTreeSet<AddressClass>>>,
}

impl RebindGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the classes a host resolved to without judging them.
    pub fn record(&self, host: &str, resolved: &[IpAddr]) {
        let classes: BTreeSet<AddressClass> = resolved.iter().map(|ip| classify(*ip)).collect();
        if classes.is_empty() {
            return;
        }
        let mut seen = self.seen.write();
        seen.entry(host.to_ascii_lowercase())
            .or_default()
            .extend(classes);
    }

    /// Classes previously seen for a host, if any.
    pub fn classes(&self, host: &str) -> Option<BTreeSet<AddressClass>> {
        self.seen.read().get(&host.to_ascii_lowercase()).cloned()
    }

    pub fn forget(&self, host: &str) {
        self.seen.write().remove(&host.to_ascii_lowercase());
    }

    pub fn clear(&self) {
        self.seen.write().clear();
    }

    /// Checks a fresh DNS answer against what the host resolved to before and
    /// records it. A host that only ever resolved publicly and now resolves to
    /// a local address is a rebinding attempt.
    pub fn check(&self, host: &str, resolved: &[IpAddr], policy: &DestinationPolicy) -> Result<()> {
        if policy.allow_private_network {
            self.record(host, resolved);
            return Ok(());
        }
        let key = host.to_ascii_lowercase();
        let previous = self.seen.read().get(&key).cloned();
        if let Some(previous) = previous
            && previous.iter().all(|class| class.is_public())
            && let Some(ip) = resolved.iter().find(|ip| classify(**ip).is_local())
        {
            return Err(denied(
                ErrorName::AccessDenied,
                format!(
                    "{host} resolved to public addresses before and to {ip} now (DNS rebinding)"
                ),
            ));
        }
        self.record(host, resolved);
        Ok(())
    }
}

/// The host of a URL as an [`IpAddr`] when it is a literal address.
pub fn literal_address(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(v4) => Some(IpAddr::V4(v4)),
        Host::Ipv6(v6) => Some(IpAddr::V6(v6)),
        Host::Domain(_) => None,
    }
}

#[cfg(test)]
mod tests;
