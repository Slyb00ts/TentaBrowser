use std::time::Duration;

use time::macros::datetime;

use super::*;

fn url(text: &str) -> Url {
    Url::parse(text).expect("test url")
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

use std::time::UNIX_EPOCH;

fn parse(header: &str, at: &str) -> Option<Cookie> {
    parse_set_cookie(header.as_bytes(), &url(at), now())
}

const HTTPS: &str = "https://www.example.com/dir/page.html";
const HTTP: &str = "http://www.example.com/dir/page.html";

// ---------------------------------------------------------------- acceptance

#[test]
fn set_cookie_acceptance_table() {
    /// `(case, Set-Cookie, request url, Some((name, value)) when accepted)`
    type Case<'a> = (&'a str, &'a str, &'a str, Option<(&'a str, &'a str)>);
    let cases: &[Case<'_>] = &[
        ("simple", "a=b", HTTPS, Some(("a", "b"))),
        ("trailing semicolon", "a=b;", HTTPS, Some(("a", "b"))),
        ("space around name", "  a  =  b  ", HTTPS, Some(("a", "b"))),
        ("tab around value", "a=\tb\t", HTTPS, Some(("a", "b"))),
        ("empty value", "a=", HTTPS, Some(("a", ""))),
        ("empty name", "=b", HTTPS, Some(("", "b"))),
        ("no equals sign", "abc", HTTPS, None),
        ("empty header", "", HTTPS, None),
        ("only equals", "=", HTTPS, None),
        ("only semicolons", ";;;", HTTPS, None),
        ("value with equals", "a=b=c", HTTPS, Some(("a", "b=c"))),
        ("value with spaces", "a=b c", HTTPS, Some(("a", "b c"))),
        ("quoted value kept", "a=\"b\"", HTTPS, Some(("a", "\"b\""))),
        (
            "value stops at semicolon",
            "a=b; c=d",
            HTTPS,
            Some(("a", "b")),
        ),
        (
            "attribute-looking value",
            "a=Path",
            HTTPS,
            Some(("a", "Path")),
        ),
        ("nul in value", "a=b\0c", HTTPS, None),
        ("newline in value", "a=b\nc", HTTPS, None),
        ("carriage return in name", "a\rb=c", HTTPS, None),
        ("del in value", "a=b\u{7f}c", HTTPS, None),
        (
            "tab inside value is allowed",
            "a=b\tc",
            HTTPS,
            Some(("a", "b\tc")),
        ),
        ("utf8 value", "a=ünïcøde", HTTPS, Some(("a", "ünïcøde"))),
        ("no host url", "a=b", "data:text/plain,x", None),
    ];
    for (case, header, at, expected) in cases {
        let parsed = parse(header, at);
        match expected {
            Some((name, value)) => {
                let cookie = parsed.unwrap_or_else(|| panic!("{case}: expected a cookie"));
                assert_eq!(
                    (cookie.name.as_str(), cookie.value.as_str()),
                    (*name, *value),
                    "{case}"
                );
            }
            None => assert!(parsed.is_none(), "{case}: expected rejection"),
        }
    }
}

#[test]
fn size_limits() {
    let long_value = "v".repeat(MAX_NAME_VALUE_BYTES - 1);
    let cookie = parse(&format!("n={long_value}"), HTTPS).expect("at the limit");
    assert_eq!(cookie.value.len(), MAX_NAME_VALUE_BYTES - 1);

    let too_long = "v".repeat(MAX_NAME_VALUE_BYTES);
    assert!(
        parse(&format!("n={too_long}"), HTTPS).is_none(),
        "over the limit"
    );

    // The limit is on name plus value together.
    let half = "x".repeat(MAX_NAME_VALUE_BYTES / 2);
    assert!(parse(&format!("{half}={half}"), HTTPS).is_some());
    assert!(parse(&format!("{half}x={half}"), HTTPS).is_none());

    // An oversized attribute value drops the attribute, not the cookie.
    let long_path = format!("/{}", "p".repeat(MAX_ATTRIBUTE_BYTES));
    let cookie = parse(&format!("a=b; Path={long_path}"), HTTPS).expect("cookie survives");
    assert_eq!(
        cookie.path, "/dir",
        "oversized Path falls back to the default"
    );
    let ok_path = format!("/{}", "p".repeat(MAX_ATTRIBUTE_BYTES - 1));
    let cookie = parse(&format!("a=b; Path={ok_path}"), HTTPS).expect("cookie");
    assert_eq!(cookie.path, ok_path);
}

// --------------------------------------------------------------------- dates

fn utc(stamp: time::OffsetDateTime) -> SystemTime {
    SystemTime::from(stamp)
}

#[test]
fn cookie_date_table() {
    /// `(case, value, expected)`
    type Case<'a> = (&'a str, &'a str, Option<SystemTime>);
    let cases: &[Case<'_>] = &[
        (
            "imf-fixdate",
            "Sun, 06 Nov 1994 08:49:37 GMT",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        (
            "rfc850",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        (
            "asctime",
            "Sun Nov  6 08:49:37 1994",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        (
            "no weekday",
            "06 Nov 1994 08:49:37 GMT",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        (
            "reordered tokens",
            "1994 Nov 06 08:49:37",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        ("epoch", "Thu, 01 Jan 1970 00:00:00 GMT", Some(UNIX_EPOCH)),
        (
            "two digit year 94 is 1994",
            "Sun, 06-Nov-94 08:49:37 GMT",
            Some(utc(datetime!(1994-11-06 08:49:37 UTC))),
        ),
        (
            "two digit year 09 is 2009",
            "Sat, 02-May-09 23:38:25 GMT",
            Some(utc(datetime!(2009-05-02 23:38:25 UTC))),
        ),
        (
            "two digit year 69 is 2069",
            "Wed, 01-Jan-69 00:00:00 GMT",
            Some(utc(datetime!(2069-01-01 00:00:00 UTC))),
        ),
        (
            "two digit year 70 is 1970",
            "Thu, 01-Jan-70 00:00:00 GMT",
            Some(UNIX_EPOCH),
        ),
        (
            "single digit hour",
            "Wed, 09 Jun 2021 1:2:3 GMT",
            Some(utc(datetime!(2021-06-09 01:02:03 UTC))),
        ),
        (
            "month name is case insensitive",
            "Wed, 09 jUn 2021 10:18:14 GMT",
            Some(utc(datetime!(2021-06-09 10:18:14 UTC))),
        ),
        (
            "long month name",
            "Wed, 09 June 2021 10:18:14 GMT",
            Some(utc(datetime!(2021-06-09 10:18:14 UTC))),
        ),
        (
            "extra trailing tokens",
            "Wed, 09 Jun 2021 10:18:14 GMT nonsense",
            Some(utc(datetime!(2021-06-09 10:18:14 UTC))),
        ),
        (
            "no timezone",
            "Wed, 09 Jun 2021 10:18:14",
            Some(utc(datetime!(2021-06-09 10:18:14 UTC))),
        ),
        (
            "december",
            "Fri, 31 Dec 2027 23:59:59 GMT",
            Some(utc(datetime!(2027-12-31 23:59:59 UTC))),
        ),
        ("empty", "", None),
        ("no digits", "not a date", None),
        ("time only", "10:18:14", None),
        ("no month", "1994-11-06 08:49:37", None),
        ("no time", "Wed, 09 Jun 2021 GMT", None),
        ("day zero", "Wed, 00 Jun 2021 10:18:14 GMT", None),
        ("day 32", "Wed, 32 Jun 2021 10:18:14 GMT", None),
        ("hour 24", "Wed, 09 Jun 2021 24:18:14 GMT", None),
        ("minute 60", "Wed, 09 Jun 2021 10:60:14 GMT", None),
        ("second 60", "Wed, 09 Jun 2021 10:18:60 GMT", None),
        ("year before 1601", "Wed, 09 Jun 1500 10:18:14 GMT", None),
        ("day not in month", "Wed, 31 Jun 2021 10:18:14 GMT", None),
        ("five digit year", "Wed, 09 Jun 12021 10:18:14 GMT", None),
    ];
    for (case, value, expected) in cases {
        assert_eq!(parse_cookie_date(value), *expected, "{case}: {value:?}");
    }
}

#[test]
fn well_formed_http_dates_agree_with_httpdate() {
    for value in [
        "Sun, 06 Nov 1994 08:49:37 GMT",
        "Tue, 15 Nov 1994 08:12:31 GMT",
        "Wed, 09 Jun 2021 10:18:14 GMT",
        "Thu, 01 Jan 1970 00:00:00 GMT",
        "Mon, 31 Dec 2035 23:59:59 GMT",
    ] {
        assert_eq!(
            parse_cookie_date(value),
            httpdate::parse_http_date(value).ok(),
            "{value}"
        );
    }
}

#[test]
fn expires_and_max_age() {
    let cookie = parse("a=b; Expires=Wed, 09 Jun 2021 10:18:14 GMT", HTTPS).expect("cookie");
    assert_eq!(
        cookie.expires,
        Some(utc(datetime!(2021-06-09 10:18:14 UTC)))
    );
    assert!(cookie.persistent);

    let cookie = parse("a=b", HTTPS).expect("cookie");
    assert_eq!(cookie.expires, None);
    assert!(!cookie.persistent);

    let cookie = parse("a=b; Max-Age=60", HTTPS).expect("cookie");
    assert_eq!(cookie.expires, Some(now() + Duration::from_secs(60)));

    // Max-Age wins over Expires whichever order they appear in.
    let cookie = parse(
        "a=b; Expires=Wed, 09 Jun 2031 10:18:14 GMT; Max-Age=60",
        HTTPS,
    )
    .expect("cookie");
    assert_eq!(cookie.expires, Some(now() + Duration::from_secs(60)));
    let cookie = parse(
        "a=b; Max-Age=60; Expires=Wed, 09 Jun 2031 10:18:14 GMT",
        HTTPS,
    )
    .expect("cookie");
    assert_eq!(cookie.expires, Some(now() + Duration::from_secs(60)));

    // Non-positive Max-Age expires the cookie immediately.
    for value in ["0", "-1", "-99999"] {
        let cookie = parse(&format!("a=b; Max-Age={value}"), HTTPS).expect("cookie");
        assert_eq!(cookie.expires, Some(UNIX_EPOCH), "Max-Age={value}");
        assert!(cookie.is_expired(now()));
    }

    // A malformed Max-Age is ignored, leaving a session cookie.
    for value in ["", "abc", "1.5", "+5", "5abc", "-"] {
        let cookie = parse(&format!("a=b; Max-Age={value}"), HTTPS).expect("cookie");
        assert_eq!(cookie.expires, None, "Max-Age={value:?}");
    }

    // An absurd Max-Age is clamped, not overflowed.
    let cookie = parse("a=b; Max-Age=99999999999999999999999", HTTPS).expect("cookie");
    assert!(cookie.expires.expect("expiry") > now() + Duration::from_secs(365 * 24 * 3600));

    // A malformed Expires is ignored.
    let cookie = parse("a=b; Expires=not-a-date", HTTPS).expect("cookie");
    assert_eq!(cookie.expires, None);
}

// -------------------------------------------------------------------- domain

#[test]
fn domain_attribute_table() {
    /// `(case, Set-Cookie, request url, Some((domain, host_only)) when accepted)`
    type Case<'a> = (&'a str, &'a str, &'a str, Option<(&'a str, bool)>);
    let cases: &[Case<'_>] = &[
        (
            "no attribute is host only",
            "a=b",
            HTTPS,
            Some(("www.example.com", true)),
        ),
        (
            "exact host",
            "a=b; Domain=www.example.com",
            HTTPS,
            Some(("www.example.com", false)),
        ),
        (
            "leading dot removed",
            "a=b; Domain=.example.com",
            HTTPS,
            Some(("example.com", false)),
        ),
        (
            "parent domain",
            "a=b; Domain=example.com",
            HTTPS,
            Some(("example.com", false)),
        ),
        (
            "case folded",
            "a=b; Domain=EXAMPLE.COM",
            HTTPS,
            Some(("example.com", false)),
        ),
        ("trailing dot host", "a=b; Domain=example.com.", HTTPS, None),
        ("sibling domain", "a=b; Domain=other.com", HTTPS, None),
        (
            "subdomain of the host",
            "a=b; Domain=deep.www.example.com",
            HTTPS,
            None,
        ),
        (
            "suffix without a dot boundary",
            "a=b; Domain=ample.com",
            HTTPS,
            None,
        ),
        ("public suffix com", "a=b; Domain=com", HTTPS, None),
        (
            "public suffix co.uk",
            "a=b; Domain=co.uk",
            "https://www.example.co.uk/",
            None,
        ),
        (
            "registrable domain under co.uk",
            "a=b; Domain=example.co.uk",
            "https://www.example.co.uk/",
            Some(("example.co.uk", false)),
        ),
        (
            "public suffix equal to the host is host only",
            "a=b; Domain=localhost",
            "http://localhost/x",
            Some(("localhost", true)),
        ),
        (
            "ip host with a domain attribute",
            "a=b; Domain=127.0.0.1",
            "http://127.0.0.1/x",
            Some(("127.0.0.1", true)),
        ),
        (
            "ip host without a domain attribute",
            "a=b",
            "http://127.0.0.1/x",
            Some(("127.0.0.1", true)),
        ),
        (
            "ip host cannot take a name domain",
            "a=b; Domain=example.com",
            "http://93.184.216.34/x",
            None,
        ),
        (
            "empty domain attribute is ignored",
            "a=b; Domain=",
            HTTPS,
            Some(("www.example.com", true)),
        ),
        ("dot only", "a=b; Domain=.", HTTPS, None),
    ];
    for (case, header, at, expected) in cases {
        let parsed = parse(header, at);
        match expected {
            Some((domain, host_only)) => {
                let cookie = parsed.unwrap_or_else(|| panic!("{case}: expected a cookie"));
                assert_eq!(cookie.domain, *domain, "{case}");
                assert_eq!(cookie.host_only, *host_only, "{case}");
            }
            None => assert!(parsed.is_none(), "{case}: expected rejection"),
        }
    }
}

#[test]
fn public_suffix_checks() {
    for suffix in ["com", "co.uk", "org", "github.io", "localhost", "local"] {
        assert!(is_public_suffix(suffix), "{suffix} must be a public suffix");
    }
    for domain in [
        "example.com",
        "example.co.uk",
        "user.github.io",
        "127.0.0.1",
        "",
    ] {
        assert!(
            !is_public_suffix(domain),
            "{domain} must not be a public suffix"
        );
    }
}

#[test]
fn domain_and_path_matching() {
    assert!(domain_matches("www.example.com", "example.com"));
    assert!(domain_matches("example.com", "example.com"));
    assert!(!domain_matches("ample.com", "example.com"));
    assert!(!domain_matches("example.com", "www.example.com"));
    assert!(!domain_matches("127.0.0.1", "0.0.1"));

    assert!(path_matches("/dir/page", "/dir"));
    assert!(path_matches("/dir", "/dir"));
    assert!(path_matches("/dir/", "/dir/"));
    assert!(path_matches("/dir/page", "/"));
    assert!(!path_matches("/directory", "/dir"));
    assert!(!path_matches("/", "/dir"));
}

// ---------------------------------------------------------------------- path

#[test]
fn path_attribute_table() {
    /// `(case, Set-Cookie, request url, expected path)`
    type Case<'a> = (&'a str, &'a str, &'a str, &'a str);
    let cases: &[Case<'_>] = &[
        ("default path drops the last segment", "a=b", HTTPS, "/dir"),
        (
            "default path at the root",
            "a=b",
            "https://example.com/",
            "/",
        ),
        (
            "default path with no path",
            "a=b",
            "https://example.com",
            "/",
        ),
        (
            "default path one level",
            "a=b",
            "https://example.com/page",
            "/",
        ),
        (
            "default path two levels",
            "a=b",
            "https://example.com/a/b/c",
            "/a/b",
        ),
        ("explicit path", "a=b; Path=/x/y", HTTPS, "/x/y"),
        ("relative path falls back", "a=b; Path=x/y", HTTPS, "/dir"),
        ("empty path falls back", "a=b; Path=", HTTPS, "/dir"),
        (
            "last path attribute wins",
            "a=b; Path=/x; Path=/y",
            HTTPS,
            "/y",
        ),
        (
            "last path attribute may reset",
            "a=b; Path=/x; Path=nope",
            HTTPS,
            "/dir",
        ),
        (
            "query is not part of the path",
            "a=b",
            "https://example.com/a/b?c=d",
            "/a",
        ),
    ];
    for (case, header, at, expected) in cases {
        let cookie = parse(header, at).unwrap_or_else(|| panic!("{case}: expected a cookie"));
        assert_eq!(cookie.path, *expected, "{case}");
    }
}

// ------------------------------------------------------------------ prefixes

#[test]
fn prefix_and_secure_rules_table() {
    /// `(case, Set-Cookie, request url, accepted)`
    type Case<'a> = (&'a str, &'a str, &'a str, bool);
    let cases: &[Case<'_>] = &[
        ("secure over https", "a=b; Secure", HTTPS, true),
        ("secure over http is refused", "a=b; Secure", HTTP, false),
        (
            "secure over http localhost",
            "a=b; Secure",
            "http://localhost/x",
            true,
        ),
        (
            "__Secure- needs the attribute",
            "__Secure-a=b",
            HTTPS,
            false,
        ),
        (
            "__Secure- with the attribute",
            "__Secure-a=b; Secure",
            HTTPS,
            true,
        ),
        ("__Secure- over http", "__Secure-a=b; Secure", HTTP, false),
        (
            "__Secure- is case insensitive",
            "__secure-a=b",
            HTTPS,
            false,
        ),
        ("__Host- needs secure", "__Host-a=b; Path=/", HTTPS, false),
        ("__Host- needs path /", "__Host-a=b; Secure", HTTPS, false),
        (
            "__Host- needs no domain",
            "__Host-a=b; Secure; Path=/; Domain=example.com",
            HTTPS,
            false,
        ),
        (
            "__Host- complete",
            "__Host-a=b; Secure; Path=/",
            HTTPS,
            true,
        ),
        (
            "__Host- over http",
            "__Host-a=b; Secure; Path=/",
            HTTP,
            false,
        ),
        (
            "__Host- is case insensitive",
            "__hOsT-a=b; Path=/",
            HTTPS,
            false,
        ),
        ("plain name over http", "a=b", HTTP, true),
        (
            "SameSite=None needs secure",
            "a=b; SameSite=None",
            HTTPS,
            false,
        ),
        (
            "SameSite=None with secure",
            "a=b; SameSite=None; Secure",
            HTTPS,
            true,
        ),
        ("Partitioned needs secure", "a=b; Partitioned", HTTPS, false),
        (
            "Partitioned with secure",
            "a=b; Secure; Partitioned",
            HTTPS,
            true,
        ),
    ];
    for (case, header, at, accepted) in cases {
        assert_eq!(parse(header, at).is_some(), *accepted, "{case}");
    }
}

#[test]
fn host_prefix_cookie_is_host_only() {
    let cookie = parse("__Host-id=1; Secure; Path=/", HTTPS).expect("cookie");
    assert!(cookie.host_only);
    assert_eq!(cookie.domain, "www.example.com");
    assert_eq!(cookie.path, "/");
    assert!(cookie.secure);
}

// ------------------------------------------------------------------ samesite

#[test]
fn same_site_attribute_table() {
    /// `(case, Set-Cookie, expected)`
    type Case<'a> = (&'a str, &'a str, SameSite);
    let cases: &[Case<'_>] = &[
        ("absent defaults to Lax", "a=b", SameSite::Lax),
        ("Strict", "a=b; SameSite=Strict", SameSite::Strict),
        ("lowercase strict", "a=b; samesite=strict", SameSite::Strict),
        ("Lax", "a=b; SameSite=Lax", SameSite::Lax),
        (
            "None with Secure",
            "a=b; SameSite=None; Secure",
            SameSite::None,
        ),
        (
            "unrecognised falls back to Lax",
            "a=b; SameSite=Nonsense",
            SameSite::Lax,
        ),
        ("empty falls back to Lax", "a=b; SameSite=", SameSite::Lax),
        (
            "valueless falls back to Lax",
            "a=b; SameSite",
            SameSite::Lax,
        ),
        (
            "last one wins",
            "a=b; SameSite=Strict; SameSite=Lax",
            SameSite::Lax,
        ),
    ];
    for (case, header, expected) in cases {
        let cookie = parse(header, HTTPS).unwrap_or_else(|| panic!("{case}: expected a cookie"));
        assert_eq!(cookie.same_site, *expected, "{case}");
    }
}

#[test]
fn http_only_attribute() {
    assert!(parse("a=b; HttpOnly", HTTPS).expect("cookie").http_only);
    assert!(parse("a=b; httponly", HTTPS).expect("cookie").http_only);
    assert!(
        parse("a=b; HttpOnly=whatever", HTTPS)
            .expect("cookie")
            .http_only
    );
    assert!(!parse("a=b", HTTPS).expect("cookie").http_only);
}

// ---------------------------------------------------------------------- site

#[test]
fn sites_and_partition_keys() {
    let site = Site::from_url(&url("https://a.b.example.co.uk/x")).expect("site");
    assert_eq!(site.scheme, "https");
    assert_eq!(site.registrable_domain, "example.co.uk");
    assert_eq!(site.to_string(), "https://example.co.uk");

    assert_eq!(
        Site::from_url(&url("wss://x.example.com/")).map(|s| s.scheme),
        Some("https".to_string())
    );
    assert_eq!(
        Site::from_url(&url("http://127.0.0.1:8080/")).map(|s| s.registrable_domain),
        Some("127.0.0.1".to_string())
    );
    assert_eq!(Site::from_url(&url("data:text/plain,x")), None);

    let a = Site::from_url(&url("https://www.example.com/")).unwrap();
    let b = Site::from_url(&url("http://api.example.com/")).unwrap();
    assert!(a.same_registrable_domain(&b));
    let c = Site::from_url(&url("https://other.com/")).unwrap();
    assert!(!a.same_registrable_domain(&c));

    let key = PartitionKey::from_url(&url("https://top.example/")).unwrap();
    assert_eq!(key.to_string(), "https://top.example");
    assert_eq!(key.site().registrable_domain, "top.example");
}

#[test]
fn partitioned_cookies_carry_the_top_level_site() {
    let top = Site::from_url(&url("https://top.example/")).unwrap();
    let cookie = parse_set_cookie_in(
        b"a=b; Secure; SameSite=None; Partitioned",
        &url("https://embed.example/x"),
        now(),
        Some(&top),
    )
    .expect("cookie");
    assert_eq!(cookie.partition_key, Some(PartitionKey(top)));

    // Without the attribute the cookie is unpartitioned even in a frame.
    let top = Site::from_url(&url("https://top.example/")).unwrap();
    let cookie = parse_set_cookie_in(
        b"a=b; Secure; SameSite=None",
        &url("https://embed.example/x"),
        now(),
        Some(&top),
    )
    .expect("cookie");
    assert_eq!(cookie.partition_key, None);
}

// ----------------------------------------------------------------------- jar

fn store(jar: &CookieJar, header: &str, at: &str, when: SystemTime) -> StoreOutcome {
    let cookie = parse_set_cookie(header.as_bytes(), &url(at), when).expect("parsed cookie");
    jar.store(cookie, &url(at), true)
}

fn names(cookies: &[Cookie]) -> Vec<&str> {
    cookies.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn jar_round_trip() {
    let jar = CookieJar::new();
    assert!(jar.is_empty());
    assert_eq!(store(&jar, "sid=1", HTTPS, now()), StoreOutcome::Stored);
    assert_eq!(jar.len(), 1);

    let sent = jar.cookies_for(&url(HTTPS), None, true, SameSiteContext::same_site(), now());
    assert_eq!(names(&sent), ["sid"]);
    assert_eq!(header_value(&sent), "sid=1");
    assert_eq!(
        jar.cookie_header(&url(HTTPS), None, true, SameSiteContext::same_site(), now()),
        Some("sid=1".to_string())
    );

    // Replacing keeps the original creation time, which decides Cookie order.
    let later = now() + Duration::from_secs(10);
    assert_eq!(store(&jar, "sid=2", HTTPS, later), StoreOutcome::Replaced);
    assert_eq!(jar.len(), 1);
    let sent = jar.cookies_for(&url(HTTPS), None, true, SameSiteContext::same_site(), later);
    assert_eq!(sent[0].value, "2");
    assert_eq!(sent[0].creation, now());
}

#[test]
fn jar_scoping_by_domain_and_path() {
    let jar = CookieJar::new();
    store(&jar, "host=1", "https://www.example.com/dir/x", now());
    store(
        &jar,
        "dom=1; Domain=example.com",
        "https://www.example.com/dir/x",
        now(),
    );
    store(
        &jar,
        "root=1; Path=/",
        "https://www.example.com/dir/x",
        now(),
    );

    let at_dir = jar.cookies_for(
        &url("https://www.example.com/dir/y"),
        None,
        true,
        SameSiteContext::same_site(),
        now(),
    );
    // Equal path length and equal creation time: the name breaks the tie.
    assert_eq!(names(&at_dir), ["dom", "host", "root"]);

    let at_root = jar.cookies_for(
        &url("https://www.example.com/other"),
        None,
        true,
        SameSiteContext::same_site(),
        now(),
    );
    assert_eq!(names(&at_root), ["root"]);

    // A host-only cookie is invisible on a sibling host; the domain one is not.
    let sibling = jar.cookies_for(
        &url("https://api.example.com/dir/y"),
        None,
        true,
        SameSiteContext::same_site(),
        now(),
    );
    assert_eq!(names(&sibling), ["dom"]);

    let unrelated = jar.cookies_for(
        &url("https://other.com/dir/y"),
        None,
        true,
        SameSiteContext::same_site(),
        now(),
    );
    assert!(unrelated.is_empty());
}

#[test]
fn jar_orders_by_path_length_then_creation() {
    let jar = CookieJar::new();
    store(
        &jar,
        "deep=1; Path=/a/b/c",
        "https://example.com/a/b/c/x",
        now(),
    );
    store(
        &jar,
        "mid=1; Path=/a/b",
        "https://example.com/a/b/c/x",
        now() + Duration::from_secs(1),
    );
    store(
        &jar,
        "shallow=1; Path=/",
        "https://example.com/a/b/c/x",
        now() + Duration::from_secs(2),
    );
    store(
        &jar,
        "mid2=1; Path=/a/b",
        "https://example.com/a/b/c/x",
        now() + Duration::from_secs(3),
    );

    let sent = jar.cookies_for(
        &url("https://example.com/a/b/c/x"),
        None,
        true,
        SameSiteContext::same_site(),
        now() + Duration::from_secs(4),
    );
    assert_eq!(names(&sent), ["deep", "mid", "mid2", "shallow"]);
    assert_eq!(header_value(&sent), "deep=1; mid=1; mid2=1; shallow=1");
}

#[test]
fn secure_and_http_only_gating() {
    let jar = CookieJar::new();
    store(&jar, "s=1; Secure", HTTPS, now());
    store(&jar, "h=1; HttpOnly", HTTPS, now());
    store(&jar, "plain=1", HTTPS, now());

    let over_https = jar.cookies_for(&url(HTTPS), None, true, SameSiteContext::same_site(), now());
    assert_eq!(names(&over_https), ["h", "plain", "s"]);

    let over_http = jar.cookies_for(&url(HTTP), None, true, SameSiteContext::same_site(), now());
    assert_eq!(names(&over_http), ["h", "plain"]);

    let from_script = jar.cookies_for(
        &url(HTTPS),
        None,
        false,
        SameSiteContext::same_site(),
        now(),
    );
    assert_eq!(names(&from_script), ["plain", "s"]);
}

#[test]
fn http_only_cannot_be_overwritten_from_script() {
    let jar = CookieJar::new();
    store(&jar, "sid=server; HttpOnly", HTTPS, now());

    let script_cookie = parse_set_cookie(b"sid=script", &url(HTTPS), now()).expect("parsed");
    assert_eq!(
        jar.store(script_cookie, &url(HTTPS), false),
        StoreOutcome::RejectedHttpOnly
    );
    assert_eq!(jar.cookies()[0].value, "server");

    // A script may not create an HttpOnly cookie either.
    let script_cookie = parse_set_cookie(b"other=1; HttpOnly", &url(HTTPS), now()).expect("parsed");
    assert_eq!(
        jar.store(script_cookie, &url(HTTPS), false),
        StoreOutcome::RejectedHttpOnly
    );
    assert_eq!(jar.len(), 1);
}

#[test]
fn same_site_contexts() {
    let jar = CookieJar::new();
    store(&jar, "strict=1; SameSite=Strict", HTTPS, now());
    store(&jar, "lax=1; SameSite=Lax", HTTPS, now());
    store(&jar, "none=1; SameSite=None; Secure", HTTPS, now());
    store(&jar, "default=1", HTTPS, now());

    let send = |context| {
        let mut sent = names(&jar.cookies_for(&url(HTTPS), None, true, context, now()))
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        sent.sort();
        sent
    };

    assert_eq!(
        send(SameSiteContext::same_site()),
        ["default", "lax", "none", "strict"]
    );
    assert_eq!(send(SameSiteContext::cross_site()), ["none"]);
    assert_eq!(
        send(SameSiteContext::cross_site_navigation()),
        ["default", "lax", "none"]
    );
    assert_eq!(
        send(SameSiteContext::cross_site_unsafe_navigation()),
        ["none"]
    );
}

#[test]
fn block_third_party_policy() {
    let jar = CookieJar::new();
    jar.set_policy(CookiePolicy::BlockThirdParty);
    assert_eq!(jar.policy(), CookiePolicy::BlockThirdParty);
    store(
        &jar,
        "id=1; SameSite=None; Secure",
        "https://tracker.example/x",
        now(),
    );

    let first_party = Site::from_url(&url("https://tracker.example/")).unwrap();
    let third_party = Site::from_url(&url("https://publisher.example/")).unwrap();
    let request = url("https://tracker.example/pixel");

    assert_eq!(
        names(&jar.cookies_for(
            &request,
            Some(&first_party),
            true,
            SameSiteContext::same_site(),
            now()
        )),
        ["id"]
    );
    assert!(
        jar.cookies_for(
            &request,
            Some(&third_party),
            true,
            SameSiteContext::cross_site(),
            now()
        )
        .is_empty()
    );

    jar.set_policy(CookiePolicy::AllowAll);
    assert_eq!(
        names(&jar.cookies_for(
            &request,
            Some(&third_party),
            true,
            SameSiteContext::cross_site(),
            now()
        )),
        ["id"]
    );
}

#[test]
fn partitioned_cookies_are_keyed_by_top_level_site() {
    let jar = CookieJar::new();
    let top_a = Site::from_url(&url("https://a.example/")).unwrap();
    let top_b = Site::from_url(&url("https://b.example/")).unwrap();
    let embed = url("https://embed.example/x");

    jar.set_cookie_in(
        b"p=1; Secure; SameSite=None; Partitioned",
        &embed,
        true,
        now(),
        Some(&top_a),
    )
    .expect("stored");
    jar.set_cookie_in(
        b"u=1; Secure; SameSite=None",
        &embed,
        true,
        now(),
        Some(&top_a),
    )
    .expect("stored");

    let under_a = jar.cookies_for(
        &embed,
        Some(&top_a),
        true,
        SameSiteContext::cross_site(),
        now(),
    );
    let mut under_a = names(&under_a);
    under_a.sort();
    assert_eq!(under_a, ["p", "u"]);

    let under_b = jar.cookies_for(
        &embed,
        Some(&top_b),
        true,
        SameSiteContext::cross_site(),
        now(),
    );
    assert_eq!(
        names(&under_b),
        ["u"],
        "the partitioned cookie stays on a.example"
    );

    // The same name in two partitions are two cookies.
    jar.set_cookie_in(
        b"p=2; Secure; SameSite=None; Partitioned",
        &embed,
        true,
        now(),
        Some(&top_b),
    )
    .expect("stored");
    assert_eq!(jar.len(), 3);
    let under_b = jar.cookies_for(
        &embed,
        Some(&top_b),
        true,
        SameSiteContext::cross_site(),
        now(),
    );
    let mut sorted = under_b.clone();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(sorted[0].value, "2");
}

#[test]
fn expiry_purges_on_read_and_on_demand() {
    let jar = CookieJar::new();
    store(&jar, "short=1; Max-Age=10", HTTPS, now());
    store(&jar, "long=1; Max-Age=1000", HTTPS, now());
    assert_eq!(jar.len(), 2);

    let after = now() + Duration::from_secs(11);
    let sent = jar.cookies_for(&url(HTTPS), None, true, SameSiteContext::same_site(), after);
    assert_eq!(names(&sent), ["long"]);
    assert_eq!(jar.len(), 1);

    assert_eq!(jar.purge_expired(now() + Duration::from_secs(2000)), 1);
    assert!(jar.is_empty());
}

#[test]
fn storing_an_expired_cookie_deletes_the_existing_one() {
    let jar = CookieJar::new();
    store(&jar, "sid=1", HTTPS, now());
    assert_eq!(jar.len(), 1);
    assert_eq!(
        store(&jar, "sid=; Max-Age=0", HTTPS, now()),
        StoreOutcome::Expired
    );
    assert!(jar.is_empty());
}

#[test]
fn store_rejects_a_cookie_for_another_host() {
    let jar = CookieJar::new();
    let mut cookie = parse_set_cookie(b"a=b", &url("https://other.com/"), now()).expect("parsed");
    assert_eq!(
        jar.store(cookie.clone(), &url(HTTPS), true),
        StoreOutcome::RejectedDomain
    );
    cookie.host_only = false;
    cookie.domain = "example.com".into();
    assert_eq!(jar.store(cookie, &url(HTTPS), true), StoreOutcome::Stored);
}

#[test]
fn per_domain_and_total_limits_evict_least_recently_accessed() {
    let jar = CookieJar::with_limits(CookieLimits {
        max_per_domain: 3,
        max_total: 5,
    });
    let at = "https://a.example/";
    for (index, second) in (0..4).enumerate() {
        store(
            &jar,
            &format!("c{index}=1"),
            at,
            now() + Duration::from_secs(second),
        );
    }
    let mut remaining = names(&jar.cookies())
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(
        remaining,
        ["c1", "c2", "c3"],
        "the oldest access is evicted"
    );

    // Reading refreshes last access, so the next eviction picks someone else.
    let read_at = now() + Duration::from_secs(10);
    let one = jar.cookies_for(&url(at), None, true, SameSiteContext::same_site(), read_at);
    assert_eq!(one.len(), 3);
    // c1 is now the least recently accessed only if we touch c2 and c3 again.
    store(&jar, "c9=1", at, now() + Duration::from_secs(11));
    let mut remaining = names(&jar.cookies())
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(remaining.len(), 3);
    assert!(remaining.contains(&"c9".to_string()));

    // The total limit spans domains.
    let jar = CookieJar::with_limits(CookieLimits {
        max_per_domain: 10,
        max_total: 3,
    });
    for (index, host) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        store(
            &jar,
            "x=1",
            &format!("https://{host}.example/"),
            now() + Duration::from_secs(index as u64),
        );
    }
    assert_eq!(jar.len(), 3);
    let domains: Vec<String> = jar.cookies().iter().map(|c| c.domain.clone()).collect();
    assert!(!domains.contains(&"a.example".to_string()));
    assert!(domains.contains(&"e.example".to_string()));
}

#[test]
fn default_limits_match_the_spec() {
    let limits = CookieJar::new().limits();
    assert_eq!(limits.max_per_domain, 180);
    assert_eq!(limits.max_total, 3000);
}

#[test]
fn remove_clear_and_iter() {
    let jar = CookieJar::new();
    store(&jar, "a=1", HTTPS, now());
    store(&jar, "b=1; Path=/", HTTPS, now());
    assert_eq!(jar.iter().count(), 2);
    assert!(jar.remove("a", "www.example.com", "/dir"));
    assert!(!jar.remove("a", "www.example.com", "/dir"));
    assert_eq!(jar.len(), 1);
    jar.clear();
    assert!(jar.is_empty());
}

#[test]
fn session_cookies_are_dropped_on_context_close() {
    let jar = CookieJar::new();
    store(&jar, "session=1", HTTPS, now());
    store(&jar, "persistent=1; Max-Age=1000", HTTPS, now());
    assert_eq!(jar.remove_session_cookies(), 1);
    assert_eq!(names(&jar.cookies()), ["persistent"]);
}

// ------------------------------------------------------------- storage state

#[test]
fn storage_state_round_trip() {
    let jar = CookieJar::new();
    store(
        &jar,
        "sid=abc; Secure; HttpOnly; SameSite=Strict",
        HTTPS,
        now(),
    );
    store(
        &jar,
        "pref=dark; Domain=example.com; Path=/; Max-Age=3600",
        HTTPS,
        now(),
    );
    store(&jar, "tmp=1", HTTPS, now());

    let state = jar.to_storage_state();
    assert_eq!(state.cookies.len(), 3);
    assert!(state.origins.is_empty());

    let json = state.to_json().expect("json");
    let parsed = StorageState::from_json(&json).expect("parse");
    assert_eq!(parsed, state);

    let restored = CookieJar::from_storage_state(&parsed, now()).expect("restore");
    assert_eq!(restored.len(), 3);

    let mut before = jar.cookies();
    let mut after = restored.cookies();
    before.sort_by(|a, b| a.name.cmp(&b.name));
    after.sort_by(|a, b| a.name.cmp(&b.name));
    for (original, restored) in before.iter().zip(after.iter()) {
        assert_eq!(original.name, restored.name);
        assert_eq!(original.value, restored.value);
        assert_eq!(original.domain, restored.domain);
        assert_eq!(original.host_only, restored.host_only);
        assert_eq!(original.path, restored.path);
        assert_eq!(original.secure, restored.secure);
        assert_eq!(original.http_only, restored.http_only);
        assert_eq!(original.same_site, restored.same_site);
        assert_eq!(original.expires, restored.expires);
        assert_eq!(original.persistent, restored.persistent);
    }
}

#[test]
fn storage_state_uses_the_playwright_shape() {
    let jar = CookieJar::new();
    store(
        &jar,
        "sid=abc; Secure; HttpOnly; SameSite=Strict; Path=/",
        HTTPS,
        now(),
    );
    let json = jar.to_storage_state().to_json().expect("json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("value");
    let cookie = &value["cookies"][0];
    assert_eq!(cookie["name"], "sid");
    assert_eq!(cookie["value"], "abc");
    assert_eq!(cookie["domain"], "www.example.com");
    assert_eq!(cookie["path"], "/");
    assert_eq!(cookie["expires"], -1.0);
    assert_eq!(cookie["httpOnly"], true);
    assert_eq!(cookie["secure"], true);
    assert_eq!(cookie["sameSite"], "Strict");
    assert!(value["origins"].is_array());
}

#[test]
fn storage_state_marks_domain_cookies_with_a_leading_dot() {
    let jar = CookieJar::new();
    store(&jar, "d=1; Domain=example.com", HTTPS, now());
    let state = jar.to_storage_state();
    assert_eq!(state.cookies[0].domain, ".example.com");

    let restored = CookieJar::from_storage_state(&state, now()).expect("restore");
    let cookie = &restored.cookies()[0];
    assert_eq!(cookie.domain, "example.com");
    assert!(!cookie.host_only);
}

#[test]
fn storage_state_accepts_playwright_json() {
    let json = r#"{
        "cookies": [
            {"name":"a","value":"1","domain":".example.com","path":"/","expires":-1,
             "httpOnly":false,"secure":true,"sameSite":"None"},
            {"name":"b","value":"2","domain":"www.example.com","path":"/x","expires":4102444800,
             "httpOnly":true,"secure":false,"sameSite":"Lax"}
        ],
        "origins": []
    }"#;
    let state = StorageState::from_json(json).expect("parse");
    let jar = CookieJar::from_storage_state(&state, now()).expect("restore");
    assert_eq!(jar.len(), 2);
    let mut cookies = jar.cookies();
    cookies.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(cookies[0].same_site, SameSite::None);
    assert!(!cookies[0].host_only);
    assert!(cookies[1].persistent);
    assert_eq!(cookies[1].path, "/x");
}

#[test]
fn storage_state_rejects_a_cookie_without_a_domain() {
    let state = StorageState {
        cookies: vec![StorageCookie {
            name: "a".into(),
            value: "1".into(),
            domain: String::new(),
            path: "/".into(),
            expires: -1.0,
            http_only: false,
            secure: false,
            same_site: "Lax".into(),
        }],
        origins: Vec::new(),
    };
    let error = CookieJar::from_storage_state(&state, now()).unwrap_err();
    assert!(matches!(error, tenta_core::Error::InvalidArgument { .. }));

    assert!(StorageState::from_json("not json").is_err());
}

#[test]
fn storage_state_skips_expired_cookies() {
    let state = StorageState {
        cookies: vec![StorageCookie {
            name: "old".into(),
            value: "1".into(),
            domain: "example.com".into(),
            path: "/".into(),
            expires: 1.0,
            http_only: false,
            secure: false,
            same_site: "Lax".into(),
        }],
        origins: Vec::new(),
    };
    let jar = CookieJar::from_storage_state(&state, now()).expect("restore");
    assert!(jar.is_empty());
}

#[test]
fn the_per_domain_quota_counts_the_registrable_domain_not_the_host() {
    // Chromium's quota is per eTLD+1: sub-domains share one budget, so a site
    // cannot hold a multiple of the limit by spreading cookies over hosts.
    let jar = CookieJar::with_limits(CookieLimits {
        max_per_domain: 4,
        max_total: 100,
    });
    for index in 0..4 {
        store(
            &jar,
            &format!("c{index}=1; Path=/"),
            &format!("https://a{index}.example.com/"),
            now(),
        );
    }
    assert_eq!(jar.len(), 4);

    store(&jar, "c9=1; Path=/", "https://b.example.com/", now());
    assert_eq!(jar.len(), 4, "the site's budget is not per host");

    store(&jar, "o=1; Path=/", "https://example.org/", now());
    assert_eq!(
        jar.len(),
        5,
        "another registrable domain has its own budget"
    );
}
