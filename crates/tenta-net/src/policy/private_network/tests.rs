use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().expect("test address")
}

#[test]
fn ipv4_classes() {
    let cases: &[(&str, AddressClass)] = &[
        ("0.0.0.0", AddressClass::Unspecified),
        ("127.0.0.1", AddressClass::Loopback),
        ("127.255.255.254", AddressClass::Loopback),
        ("10.0.0.1", AddressClass::Private),
        ("172.16.0.1", AddressClass::Private),
        ("172.31.255.255", AddressClass::Private),
        ("172.32.0.1", AddressClass::Public),
        ("192.168.1.1", AddressClass::Private),
        ("169.254.169.254", AddressClass::LinkLocal),
        ("100.64.0.1", AddressClass::CarrierGradeNat),
        ("100.127.255.255", AddressClass::CarrierGradeNat),
        ("100.128.0.1", AddressClass::Public),
        ("100.63.255.255", AddressClass::Public),
        ("224.0.0.1", AddressClass::Multicast),
        ("239.255.255.250", AddressClass::Multicast),
        ("192.0.2.1", AddressClass::Documentation),
        ("198.51.100.7", AddressClass::Documentation),
        ("203.0.113.9", AddressClass::Documentation),
        ("198.18.0.1", AddressClass::Benchmarking),
        ("198.19.255.255", AddressClass::Benchmarking),
        ("192.0.0.8", AddressClass::Reserved),
        ("240.0.0.1", AddressClass::Reserved),
        ("255.255.255.255", AddressClass::Broadcast),
        ("0.1.2.3", AddressClass::Reserved),
        ("8.8.8.8", AddressClass::Public),
        ("1.1.1.1", AddressClass::Public),
    ];
    for (text, expected) in cases {
        assert_eq!(classify(ip(text)), *expected, "{text}");
    }
}

#[test]
fn ipv6_classes() {
    let cases: &[(&str, AddressClass)] = &[
        ("::", AddressClass::Unspecified),
        ("::1", AddressClass::Loopback),
        ("fc00::1", AddressClass::Private),
        ("fd12:3456::1", AddressClass::Private),
        ("fe80::1", AddressClass::LinkLocal),
        ("febf::1", AddressClass::LinkLocal),
        ("fec0::1", AddressClass::Private),
        ("ff02::1", AddressClass::Multicast),
        ("2001:db8::1", AddressClass::Documentation),
        ("3fff:0100::1", AddressClass::Documentation),
        ("2001:2::1", AddressClass::Benchmarking),
        ("100::1", AddressClass::Reserved),
        ("2001:20::1", AddressClass::Reserved),
        ("2606:4700:4700::1111", AddressClass::Public),
        ("2a00:1450:4001::200e", AddressClass::Public),
    ];
    for (text, expected) in cases {
        assert_eq!(classify(ip(text)), *expected, "{text}");
    }
}

#[test]
fn ipv4_mapped_and_compatible_v6_use_the_embedded_address() {
    assert_eq!(classify(ip("::ffff:127.0.0.1")), AddressClass::Loopback);
    assert_eq!(classify(ip("::ffff:192.168.0.1")), AddressClass::Private);
    assert_eq!(classify(ip("::ffff:8.8.8.8")), AddressClass::Public);
    assert_eq!(classify(ip("::127.0.0.1")), AddressClass::Loopback);
    assert_eq!(classify(ip("::10.0.0.1")), AddressClass::Private);
}

#[test]
fn nat64_addresses_are_classified_by_the_address_they_translate_to() {
    // RFC 6052 `64:ff9b::/96`: on an IPv6-only network these reach the
    // embedded IPv4 destination through a NAT64 gateway.
    assert_eq!(classify(ip("64:ff9b::7f00:1")), AddressClass::Loopback);
    assert_eq!(classify(ip("64:ff9b::c0a8:1")), AddressClass::Private);
    assert_eq!(classify(ip("64:ff9b::a9fe:a9fe")), AddressClass::LinkLocal);
    assert_eq!(classify(ip("64:ff9b::808:808")), AddressClass::Public);
    // RFC 8215 local-use translation prefix: a translator on this network.
    assert_eq!(classify(ip("64:ff9b:1::1")), AddressClass::Private);
}

#[test]
fn private_host_names() {
    let cases: &[(&str, bool)] = &[
        ("localhost", true),
        ("LOCALHOST", true),
        ("localhost.", true),
        ("api.localhost", true),
        ("printer.local", true),
        ("local", true),
        ("127.0.0.1", true),
        ("[::1]", true),
        ("::1", true),
        ("169.254.169.254", true),
        ("10.1.2.3", true),
        ("example.com", false),
        ("localhostname.com", false),
        ("notlocal", false),
        ("8.8.8.8", false),
        ("localhost.example.com", false),
    ];
    for (host, expected) in cases {
        assert_eq!(is_private_host(host), *expected, "{host}");
    }
}

fn url(text: &str) -> Url {
    Url::parse(text).expect("test url")
}

fn reason(error: &Error) -> String {
    match error {
        Error::NetDenied { reason } => reason.clone(),
        other => panic!("expected NetDenied, got {other:?}"),
    }
}

#[test]
fn restricted_ports_are_refused() {
    let policy = DestinationPolicy::default();
    for port in [1u16, 22, 25, 465, 6667, 10080] {
        let error = policy
            .check_url(&url(&format!("http://example.com:{port}/")))
            .expect_err("restricted port must be refused");
        assert!(reason(&error).starts_with("net::ERR_UNSAFE_PORT"), "{port}");
    }
    assert!(policy.check_url(&url("http://example.com:8080/")).is_ok());
    assert!(policy.check_url(&url("https://example.com/")).is_ok());
}

#[test]
fn restricted_port_list_matches_chromium_size_and_bounds() {
    assert_eq!(RESTRICTED_PORTS.len(), 82);
    assert!(RESTRICTED_PORTS.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(RESTRICTED_PORTS.first(), Some(&1));
    assert_eq!(RESTRICTED_PORTS.last(), Some(&10080));
    for port in [80u16, 443, 8080, 3000, 8000] {
        assert!(!RESTRICTED_PORTS.contains(&port), "{port} must be allowed");
    }
}

#[test]
fn schemes_outside_the_allow_list_are_blocked_by_client() {
    let policy = DestinationPolicy::default();
    let error = policy.check_url(&url("ftp://example.com/x")).unwrap_err();
    assert!(reason(&error).starts_with("net::ERR_BLOCKED_BY_CLIENT"));
    let policy = policy.allow_scheme("ftp");
    assert!(policy.check_url(&url("ftp://example.com/x")).is_ok());
}

#[test]
fn private_hosts_and_addresses_need_the_flag() {
    let policy = DestinationPolicy::default();
    let error = policy
        .check_url(&url("http://localhost:3000/"))
        .unwrap_err();
    assert!(reason(&error).starts_with("net::ERR_ACCESS_DENIED"));

    let error = policy
        .check(&url("http://example.com/"), &[ip("192.168.0.5")])
        .unwrap_err();
    assert!(reason(&error).contains("private-network"));

    assert!(
        policy
            .check(&url("http://example.com/"), &[ip("93.184.216.34")])
            .is_ok()
    );

    let permissive = DestinationPolicy::permissive();
    assert!(permissive.check_url(&url("http://localhost:3000/")).is_ok());
    assert!(
        permissive
            .check(&url("http://example.com/"), &[ip("192.168.0.5")])
            .is_ok()
    );
}

#[test]
fn any_private_address_in_the_answer_is_enough_to_refuse() {
    let policy = DestinationPolicy::default();
    let answer = [ip("93.184.216.34"), ip("127.0.0.1")];
    assert!(policy.check(&url("http://example.com/"), &answer).is_err());
}

#[test]
fn redirect_limit() {
    let policy = DestinationPolicy::default();
    assert!(policy.check_redirect(1).is_ok());
    assert!(policy.check_redirect(20).is_ok());
    let error = policy.check_redirect(21).unwrap_err();
    assert!(reason(&error).starts_with("net::ERR_TOO_MANY_REDIRECTS"));
}

#[test]
fn rebind_guard_flags_a_public_host_turning_private() {
    let policy = DestinationPolicy::default();
    let guard = RebindGuard::new();
    assert!(
        guard
            .check("attacker.example", &[ip("93.184.216.34")], &policy)
            .is_ok()
    );
    let error = guard
        .check("attacker.example", &[ip("127.0.0.1")], &policy)
        .unwrap_err();
    assert!(reason(&error).contains("DNS rebinding"));
}

#[test]
fn rebind_guard_is_per_host_and_case_insensitive() {
    let policy = DestinationPolicy::default();
    let guard = RebindGuard::new();
    guard.record("Attacker.Example", &[ip("93.184.216.34")]);
    assert!(
        guard
            .check("ATTACKER.EXAMPLE", &[ip("10.0.0.1")], &policy)
            .is_err()
    );
    assert!(
        guard
            .check("other.example", &[ip("10.0.0.1")], &policy)
            .is_ok()
    );
}

#[test]
fn rebind_guard_allows_a_host_that_was_always_private() {
    let policy = DestinationPolicy::permissive();
    let guard = RebindGuard::new();
    assert!(guard.check("dev.box", &[ip("10.0.0.1")], &policy).is_ok());
    assert!(guard.check("dev.box", &[ip("10.0.0.2")], &policy).is_ok());

    // With the flag off the first answer is recorded by the guard but the
    // destination check is what refuses it; a second private answer is not a
    // rebinding because the host was never public.
    let strict = DestinationPolicy::default();
    let guard = RebindGuard::new();
    guard.record("dev.box", &[ip("10.0.0.1")]);
    assert!(guard.check("dev.box", &[ip("10.0.0.2")], &strict).is_ok());
}

#[test]
fn rebind_guard_forget_and_clear() {
    let policy = DestinationPolicy::default();
    let guard = RebindGuard::new();
    guard.record("a.example", &[ip("93.184.216.34")]);
    assert!(guard.classes("a.example").is_some());
    guard.forget("a.example");
    assert!(guard.classes("a.example").is_none());
    assert!(
        guard
            .check("a.example", &[ip("127.0.0.1")], &policy)
            .is_ok()
    );
    guard.clear();
    assert!(guard.classes("a.example").is_none());
}

#[test]
fn literal_addresses_are_extracted() {
    assert_eq!(
        literal_address(&url("http://127.0.0.1/")),
        Some(ip("127.0.0.1"))
    );
    assert_eq!(literal_address(&url("http://[::1]/")), Some(ip("::1")));
    assert_eq!(literal_address(&url("http://example.com/")), None);
}
