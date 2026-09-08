//! The Chrome 144 profiles against the bytes they were measured from.
//!
//! `profiles/captures/chrome_144_macos/` holds a real Chrome ClientHello, HTTP/2
//! client preface and HTTP/1.1 requests, recorded by `fp-capture`. This test
//! re-derives every fingerprint in `profiles/chrome_144_*.toml` from those bytes,
//! so a hand edit of a profile — or a regression in the parsers — fails here
//! rather than silently on the wire (`docs/spec/03-network.md` §2).
//!
//! It does not test our transport; `tests/fingerprint/` does that once
//! `tenta-net` can speak.

use std::path::{Path, PathBuf};

use tenta_testutil::fingerprint::{
    ClientHello, H2Preface, akamai_h2, ext, ja4, parse_client_hello, parse_h2_preface,
};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn capture_dir() -> PathBuf {
    repository_root().join("profiles/captures/chrome_144_macos")
}

fn read_capture(name: &str) -> Vec<u8> {
    let path = capture_dir().join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

fn hello() -> ClientHello {
    parse_client_hello(&read_capture("client_hello.bin")).expect("parse captured ClientHello")
}

fn preface() -> H2Preface {
    parse_h2_preface(&read_capture("h2_preface.bin")).expect("parse captured HTTP/2 preface")
}

fn profile(name: &str) -> toml::Value {
    let path = repository_root()
        .join("profiles")
        .join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    toml::from_str(&text).expect("profile is valid TOML")
}

fn text_at<'a>(value: &'a toml::Value, section: &str, key: &str) -> &'a str {
    value[section][key]
        .as_str()
        .unwrap_or_else(|| panic!("[{section}] {key} is not a string"))
}

fn names(value: &toml::Value, section: &str, key: &str) -> Vec<String> {
    value[section][key]
        .as_array()
        .unwrap_or_else(|| panic!("[{section}] {key} is not an array"))
        .iter()
        .map(|v| v.as_str().expect("array of strings").to_owned())
        .collect()
}

const PROFILES: [&str; 3] = ["chrome_144_macos", "chrome_144_linux", "chrome_144_windows"];

#[test]
fn profile_ja4_equals_the_captured_client_hello() {
    let measured = ja4(&hello());
    for name in PROFILES {
        let profile = profile(name);
        assert_eq!(
            text_at(&profile, "meta", "ja4"),
            measured.full,
            "{name} ja4"
        );
        assert_eq!(
            text_at(&profile, "meta", "ja4_r"),
            measured.raw(),
            "{name} ja4_r"
        );
    }
    // The value itself, so a re-capture that moves it is visible in the diff.
    assert_eq!(measured.full, "t13d1516h2_8daaf6152771_d8a2da3f94cd");
}

#[test]
fn profile_akamai_equals_the_captured_preface() {
    let measured = akamai_h2(&preface());
    for name in PROFILES {
        assert_eq!(
            text_at(&profile(name), "meta", "akamai_h2"),
            measured,
            "{name} akamai_h2"
        );
    }
    assert_eq!(
        measured,
        "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p"
    );
}

#[test]
fn profile_h2_section_equals_the_captured_preface() {
    let preface = preface();
    let profile = profile("chrome_144_macos");

    let settings: Vec<(u16, u32)> = profile["h2"]["settings"]
        .as_array()
        .expect("settings array")
        .iter()
        .map(|pair| {
            let pair = pair.as_array().expect("[id, value] pair");
            (
                u16::try_from(pair[0].as_integer().expect("id")).expect("id fits u16"),
                u32::try_from(pair[1].as_integer().expect("value")).expect("value fits u32"),
            )
        })
        .collect();
    assert_eq!(settings, preface.settings);

    let window = profile["h2"]["window_update"]
        .as_integer()
        .expect("window_update");
    assert_eq!(
        u32::try_from(window).ok(),
        preface.window_update,
        "connection WINDOW_UPDATE"
    );
    assert!(
        preface.priority_frames.is_empty(),
        "Chrome sends no standalone PRIORITY frames"
    );

    let headers = preface.first_headers.as_ref().expect("first HEADERS frame");
    let pseudo: String = names(&profile, "h2", "pseudo_header_order")
        .iter()
        .map(|name| name.chars().next().expect("non-empty pseudo-header name"))
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(pseudo, headers.pseudo_header_order);

    let priority = headers.priority.expect("Chrome sets the PRIORITY flag");
    let table = &profile["h2"]["headers_priority"];
    assert_eq!(table["exclusive"].as_bool(), Some(priority.exclusive));
    assert_eq!(
        u32::try_from(table["dep"].as_integer().expect("dep")).ok(),
        Some(priority.dependency)
    );
    // The profile stores the wire weight; RFC 7540 weight is one more.
    let wire_weight = table["weight"].as_integer().expect("weight");
    assert_eq!(
        u16::try_from(wire_weight).expect("weight") + 1,
        priority.weight
    );
}

#[test]
fn profile_tls_section_equals_the_captured_client_hello() {
    let hello = hello();
    let profile = profile("chrome_144_macos");

    let ciphers: Vec<u16> = names(&profile, "tls", "ciphers")
        .iter()
        .map(|c| {
            u16::from_str_radix(c.trim_start_matches("0x"), 16).expect("hex cipher identifier")
        })
        .collect();
    assert_eq!(ciphers, hello.ciphers_without_grease(), "cipher list");

    let mut listed: Vec<u16> = profile["tls"]["extensions"]
        .as_array()
        .expect("extensions array")
        .iter()
        .map(|v| u16::try_from(v.as_integer().expect("extension id")).expect("fits u16"))
        .collect();
    let mut captured = hello.extensions_without_grease();
    // Chrome permutes, so only the set is stable.
    listed.sort_unstable();
    captured.sort_unstable();
    assert_eq!(listed, captured, "extension set");
    assert_eq!(profile["tls"]["permute_extensions"].as_bool(), Some(true));

    assert_eq!(
        names(&profile, "tls", "alpn"),
        hello
            .alpn()
            .expect("ALPN")
            .iter()
            .map(|p| String::from_utf8(p.clone()).expect("ASCII ALPN"))
            .collect::<Vec<_>>()
    );

    let alps = hello.application_settings().expect("ALPS");
    assert_eq!(
        profile["tls"]["alps_new_codepoint"].as_bool(),
        Some(alps.uses_new_codepoint())
    );
    assert_eq!(alps.codepoint, ext::APPLICATION_SETTINGS_NEW);
    assert_eq!(
        names(&profile, "tls", "alps"),
        alps.protocols
            .iter()
            .map(|p| String::from_utf8(p.clone()).expect("ASCII ALPS"))
            .collect::<Vec<_>>()
    );

    assert_eq!(
        profile["tls"]["grease"].as_bool(),
        Some(hello.uses_grease())
    );
    assert_eq!(
        profile["tls"]["ech"].as_bool(),
        Some(hello.has_extension(ext::ENCRYPTED_CLIENT_HELLO))
    );
    assert_eq!(
        profile["tls"]["signed_cert_timestamps"].as_bool(),
        Some(hello.signed_certificate_timestamp())
    );
    assert_eq!(
        profile["tls"]["ocsp_stapling"].as_bool(),
        Some(hello.status_request_type() == Some(1))
    );
    assert_eq!(
        profile["tls"]["session_tickets"].as_bool(),
        Some(hello.has_extension(ext::SESSION_TICKET))
    );
    assert_eq!(
        names(&profile, "tls", "cert_compression"),
        vec!["brotli".to_owned()],
    );
    assert_eq!(
        hello.certificate_compression_algorithms(),
        Some(vec![2]),
        "RFC 8879: 2 is brotli"
    );
}

#[test]
fn captured_client_hello_carries_the_post_quantum_key_share() {
    // X25519MLKEM768 (0x11ec) first, then X25519; GREASE leads both lists.
    let hello = hello();
    let groups = hello.supported_groups().expect("supported_groups");
    assert_eq!(
        groups
            .iter()
            .copied()
            .filter(|g| !tenta_testutil::fingerprint::is_grease(*g))
            .collect::<Vec<_>>(),
        vec![0x11ec, 0x001d, 0x0017, 0x0018]
    );
    let shares = hello.key_share_groups().expect("key_share");
    assert!(tenta_testutil::fingerprint::is_grease(shares[0]));
    assert_eq!(&shares[1..], &[0x11ec, 0x001d]);
    assert_eq!(hello.psk_key_exchange_modes(), Some(vec![1]));
    assert_eq!(hello.effective_version(), 0x0304);
    assert_eq!(hello.padding_len(), None, "the hello is already MTU-sized");
}

#[test]
fn profile_header_order_matches_both_captures() {
    let profile = profile("chrome_144_macos");
    let order = names(&profile, "headers", "order");
    let subresource = names(&profile, "headers", "subresource_order");
    let lower = |names: &[String]| -> Vec<String> {
        names.iter().map(|n| n.to_ascii_lowercase()).collect()
    };

    // HTTP/2: every request Chrome made must be a subsequence of the profile
    // list for its kind, and must contain no name the profile does not know.
    let navigation = lower(&order);
    let sub = lower(&subresource);
    for (path, expected) in [
        ("/", &navigation),
        ("/form", &navigation),
        ("/second", &navigation),
        ("/sub.css", &sub),
        ("/sub.js", &sub),
        ("/api-post", &sub),
        ("/cross-site.png", &sub),
    ] {
        let captured = captured_h2_order(path);
        assert!(
            is_subsequence(
                &captured.iter().map(String::as_str).collect::<Vec<_>>(),
                expected
            ),
            "captured HTTP/2 order of {path} {captured:?} is not a subsequence of {expected:?}"
        );
    }

    // The first HEADERS of the stored preface is the same navigation.
    let preface = preface();
    let headers = preface.first_headers.as_ref().expect("HEADERS");
    assert!(is_subsequence(
        &headers
            .header_order()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        &navigation
    ));
    assert_eq!(
        headers.header_order().last().map(String::as_str),
        Some("priority"),
        "Chrome ends an HTTP/2 request with the priority header"
    );

    // HTTP/1.1 over TLS: a prefix-preserving subset with the captured casing.
    let request = String::from_utf8(read_capture("https_http1_request.txt")).expect("UTF-8");
    let sent: Vec<&str> = request
        .lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.split(':').next().expect("header name"))
        .collect();
    assert!(
        is_subsequence(&sent, &order),
        "captured HTTP/1.1 order {sent:?} is not a subsequence of the profile order {order:?}"
    );
    assert!(
        !sent.contains(&"priority"),
        "the priority header is HTTP/2 only"
    );

    // The sub-resource request is where the other HTTP/1.1 order was measured.
    let sub_request =
        String::from_utf8(read_capture("https_http1_subresource.txt")).expect("UTF-8");
    let sub_sent: Vec<&str> = sub_request
        .lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.split(':').next().expect("header name"))
        .collect();
    assert!(
        is_subsequence(&sub_sent, &subresource),
        "captured sub-resource order {sub_sent:?} is not a subsequence of {subresource:?}"
    );
    assert!(
        sub_request.trim_end().ends_with("Cookie: fp=1"),
        "Cookie is the last header Chrome sends on HTTP/1.1"
    );
}

#[test]
fn profile_priority_table_equals_the_capture() {
    let profile = profile("chrome_144_macos");
    for (path, key) in [
        ("/", "document"),
        ("/sub.css", "stylesheet"),
        ("/sub.js", "blocking_script"),
        ("/sub.png", "image"),
        ("/api", "fetch"),
        ("/sub.woff2", "font"),
    ] {
        let captured = captured_h2_header(path, "priority")
            .unwrap_or_else(|| panic!("no priority header captured for {path}"));
        assert_eq!(
            profile["headers"]["priority"][key].as_str(),
            Some(captured.as_str()),
            "[headers.priority] {key}"
        );
    }
}

/// The header names of the captured HTTP/2 request for `path`, in wire order.
fn captured_h2_order(path: &str) -> Vec<String> {
    captured_h2_request(path)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !name.starts_with(':'))
        .collect()
}

fn captured_h2_header(path: &str, name: &str) -> Option<String> {
    captured_h2_request(path)
        .into_iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value)
}

fn captured_h2_request(path: &str) -> Vec<(String, String)> {
    let text = String::from_utf8(read_capture("h2_requests.json")).expect("UTF-8");
    let requests: serde_json::Value = serde_json::from_str(&text).expect("capture is JSON");
    requests
        .as_array()
        .expect("array of requests")
        .iter()
        .map(|request| {
            request["headers"]
                .as_array()
                .expect("headers")
                .iter()
                .map(|field| {
                    (
                        field[0].as_str().expect("name").to_owned(),
                        field[1].as_str().expect("value").to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .find(|headers| {
            headers
                .iter()
                .any(|(name, value)| name == ":path" && value == path)
        })
        .unwrap_or_else(|| panic!("the capture has no request for {path}"))
}

#[test]
fn profile_header_values_match_the_capture() {
    let preface = preface();
    let headers = preface.first_headers.as_ref().expect("HEADERS");
    let profile = profile("chrome_144_macos");
    for (key, header) in [
        ("user_agent", "user-agent"),
        ("accept", "accept"),
        ("accept_language", "accept-language"),
        ("accept_encoding", "accept-encoding"),
        ("sec_ch_ua", "sec-ch-ua"),
        ("sec_ch_ua_mobile", "sec-ch-ua-mobile"),
        ("sec_ch_ua_platform", "sec-ch-ua-platform"),
        ("upgrade_insecure_requests", "upgrade-insecure-requests"),
        ("sec_fetch_site", "sec-fetch-site"),
        ("sec_fetch_mode", "sec-fetch-mode"),
        ("sec_fetch_user", "sec-fetch-user"),
        ("sec_fetch_dest", "sec-fetch-dest"),
    ] {
        assert_eq!(
            text_at(&profile, "headers", key),
            headers.header(header).unwrap_or_default(),
            "[headers] {key}"
        );
    }
}

#[test]
fn os_profiles_differ_only_in_the_two_platform_headers() {
    let macos = profile("chrome_144_macos");
    for name in ["chrome_144_linux", "chrome_144_windows"] {
        let other = profile(name);
        assert_eq!(other["tls"], macos["tls"], "{name} [tls]");
        assert_eq!(other["h2"], macos["h2"], "{name} [h2]");
        for key in [
            "order",
            "accept",
            "accept_language",
            "accept_encoding",
            "sec_ch_ua",
        ] {
            assert_eq!(
                other["headers"][key], macos["headers"][key],
                "{name} [headers] {key}"
            );
        }
        assert_ne!(
            text_at(&other, "headers", "user_agent"),
            text_at(&macos, "headers", "user_agent")
        );
        assert_ne!(
            text_at(&other, "headers", "sec_ch_ua_platform"),
            text_at(&macos, "headers", "sec_ch_ua_platform")
        );
        assert_eq!(text_at(&other, "meta", "version"), "144");
    }
}

#[test]
fn capture_directory_holds_every_file_the_profiles_cite() {
    for name in [
        "client_hello.bin",
        "h2_preface.bin",
        "http1_request.txt",
        "https_http1_request.txt",
        "https_http1_subresource.txt",
        "h2_requests.json",
        "notes.json",
    ] {
        let path = capture_dir().join(name);
        assert!(
            is_non_empty(&path),
            "{} is missing or empty",
            path.display()
        );
    }
    // No canonical hello: Chrome 144 cannot be told to stop permuting.
    assert!(
        !capture_dir().join("client_hello_canonical.bin").exists(),
        "a canonical capture would need the profiles to gain ja3_canonical"
    );
}

fn is_non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

/// True when every element of `needle` appears in `haystack` in the same order.
fn is_subsequence(needle: &[&str], haystack: &[String]) -> bool {
    let mut haystack = haystack.iter();
    needle
        .iter()
        .all(|name| haystack.any(|candidate| candidate == name))
}
