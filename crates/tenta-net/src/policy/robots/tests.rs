//! The behavioural cases below are reimplementations of the `ID_*` and
//! `GoogleOnly_*` cases in `robots_test.cc` of
//! <https://github.com/google/robotstxt> (Apache-2.0). They are the de-facto
//! conformance suite for RFC 9309 and the behaviour site owners test against.

use super::*;

fn allowed(robots: &str, user_agent: &str, url: &str) -> bool {
    RobotsTxt::parse(robots.as_bytes()).is_allowed(user_agent, url)
}

/// `(case name, robots.txt, user agent, url, expected allowed)`
type Case<'a> = (&'a str, &'a str, &'a str, &'a str, bool);

fn run(cases: &[Case<'_>]) {
    for (name, robots, agent, url, expected) in cases {
        assert_eq!(
            allowed(robots, agent, url),
            *expected,
            "{name}: {agent} {url}\n{robots}"
        );
    }
}

const SIMPLE: &str = "user-agent: FooBot\ndisallow: /\n";

#[test]
fn id_user_agent_value_case_insensitive() {
    let upper = "User-Agent: FOO BAR\nAllow: /x/\nDisallow: /\n";
    let lower = "User-Agent: foo bar\nAllow: /x/\nDisallow: /\n";
    let mixed = "User-Agent: FoO bAr\nAllow: /x/\nDisallow: /\n";
    run(&[
        ("upper-allowed", upper, "Foo", "http://foo.bar/x/y", true),
        (
            "upper-disallowed",
            upper,
            "Foo",
            "http://foo.bar/a/b",
            false,
        ),
        ("lower-allowed", lower, "Foo", "http://foo.bar/x/y", true),
        (
            "lower-disallowed",
            lower,
            "Foo",
            "http://foo.bar/a/b",
            false,
        ),
        ("mixed-allowed", mixed, "Foo", "http://foo.bar/x/y", true),
        (
            "mixed-disallowed",
            mixed,
            "Foo",
            "http://foo.bar/a/b",
            false,
        ),
        ("agent-upper", upper, "foo", "http://foo.bar/x/y", true),
        ("agent-mixed", upper, "FOO", "http://foo.bar/x/y", true),
    ]);
}

#[test]
fn id_global_groups_secondary() {
    let global = "user-agent: *\nallow: /\nuser-agent: FooBot\ndisallow: /\n";
    let only_specific = "user-agent: FooBot\nallow: /\nuser-agent: BarBot\ndisallow: /\nuser-agent: BazBot\ndisallow: /\n";
    let url = "http://foo.bar/x/y";
    run(&[
        ("empty file allows", "", "FooBot", url, true),
        ("specific beats global", global, "FooBot", url, false),
        ("global applies to others", global, "BarBot", url, true),
        ("no group at all allows", only_specific, "QuxBot", url, true),
        ("named group applies", only_specific, "BarBot", url, false),
        ("named group applies 2", only_specific, "BazBot", url, false),
        ("named allow group", only_specific, "FooBot", url, true),
    ]);
}

#[test]
fn id_allow_disallow_value_case_sensitive() {
    let lower = "user-agent: FooBot\ndisallow: /x/\n";
    let upper = "user-agent: FooBot\ndisallow: /X/\n";
    run(&[
        (
            "path case matches",
            lower,
            "FooBot",
            "http://foo.bar/x/y",
            false,
        ),
        (
            "path case differs",
            upper,
            "FooBot",
            "http://foo.bar/x/y",
            true,
        ),
    ]);
}

#[test]
fn id_longest_match() {
    run(&[
        (
            "disallow longer",
            "user-agent: FooBot\ndisallow: /x/page.html\nallow: /x/\n",
            "FooBot",
            "http://foo.bar/x/page.html",
            false,
        ),
        (
            "allow longer",
            "user-agent: FooBot\nallow: /x/page.html\ndisallow: /x/\n",
            "FooBot",
            "http://foo.bar/x/page.html",
            true,
        ),
        (
            "allow longer, other path still blocked",
            "user-agent: FooBot\nallow: /x/page.html\ndisallow: /x/\n",
            "FooBot",
            "http://foo.bar/x/",
            false,
        ),
        (
            "empty rules allow",
            "user-agent: FooBot\ndisallow: \nallow: \n",
            "FooBot",
            "http://foo.bar/x/page.html",
            true,
        ),
        (
            "tie goes to allow",
            "user-agent: FooBot\ndisallow: /\nallow: /\n",
            "FooBot",
            "http://foo.bar/x/page.html",
            true,
        ),
        (
            "prefix without slash",
            "user-agent: FooBot\ndisallow: /x\nallow: /x/\n",
            "FooBot",
            "http://foo.bar/x",
            false,
        ),
        (
            "prefix with slash",
            "user-agent: FooBot\ndisallow: /x\nallow: /x/\n",
            "FooBot",
            "http://foo.bar/x/",
            true,
        ),
        (
            "identical patterns tie to allow",
            "user-agent: FooBot\ndisallow: /x/page.html\nallow: /x/page.html\n",
            "FooBot",
            "http://foo.bar/x/page.html",
            true,
        ),
        (
            "wildcard disallow beats shorter allow",
            "user-agent: FooBot\nallow: /page\ndisallow: /*.html\n",
            "FooBot",
            "http://foo.bar/page.html",
            false,
        ),
        (
            "wildcard disallow does not match",
            "user-agent: FooBot\nallow: /page\ndisallow: /*.html\n",
            "FooBot",
            "http://foo.bar/page",
            true,
        ),
        (
            "longer allow beats wildcard disallow",
            "user-agent: FooBot\nallow: /x/page.\ndisallow: /*.html\n",
            "FooBot",
            "http://foo.bar/x/page.html",
            true,
        ),
        (
            "wildcard disallow elsewhere",
            "user-agent: FooBot\nallow: /x/page.\ndisallow: /*.html\n",
            "FooBot",
            "http://foo.bar/x/y.html",
            false,
        ),
        (
            "named group ignores global rules",
            "User-agent: *\nDisallow: /x/\nUser-agent: FooBot\nDisallow: /y/\n",
            "FooBot",
            "http://foo.bar/x/page",
            true,
        ),
        (
            "named group rule applies",
            "User-agent: *\nDisallow: /x/\nUser-agent: FooBot\nDisallow: /y/\n",
            "FooBot",
            "http://foo.bar/y/page",
            false,
        ),
    ]);
}

#[test]
fn id_line_syntax_line() {
    let correct = "user-agent: FooBot\ndisallow: /\n";
    let incorrect = "foo: FooBot\nbar: /\n";
    let colon_free = "user-agent FooBot\ndisallow /\n";
    let url = "http://foo.bar/x/y";
    run(&[
        ("correct", correct, "FooBot", url, false),
        ("unknown keys ignored", incorrect, "FooBot", url, true),
        ("missing colon tolerated", colon_free, "FooBot", url, false),
    ]);
}

#[test]
fn id_line_syntax_groups() {
    let robots = concat!(
        "allow: /foo/bar/\n",
        "\n",
        "user-agent: FooBot\n",
        "disallow: /\n",
        "allow: /x/\n",
        "user-agent: BarBot\n",
        "disallow: /\n",
        "allow: /y/\n",
        "\n",
        "\n",
        "allow: /w/\n",
        "user-agent: BazBot\n",
        "\n",
        "user-agent: FooBot\n",
        "allow: /z/\n",
        "disallow: /\n",
    );
    run(&[
        ("foo x", robots, "FooBot", "http://foo.bar/x/b", true),
        ("foo z", robots, "FooBot", "http://foo.bar/z/d", true),
        ("foo y", robots, "FooBot", "http://foo.bar/y/c", false),
        ("bar y", robots, "BarBot", "http://foo.bar/y/c", true),
        ("bar w", robots, "BarBot", "http://foo.bar/w/a", true),
        ("bar z", robots, "BarBot", "http://foo.bar/z/d", false),
        ("baz z", robots, "BazBot", "http://foo.bar/z/d", true),
        (
            "rules before any group are ignored (foo)",
            robots,
            "FooBot",
            "http://foo.bar/foo/bar/",
            false,
        ),
        (
            "rules before any group are ignored (bar)",
            robots,
            "BarBot",
            "http://foo.bar/foo/bar/",
            false,
        ),
        (
            "rules before any group are ignored (baz)",
            robots,
            "BazBot",
            "http://foo.bar/foo/bar/",
            false,
        ),
    ]);
}

#[test]
fn id_line_syntax_groups_other_rules() {
    let with_sitemap =
        "User-agent: BarBot\nSitemap: https://foo.bar/sitemap\nUser-agent: *\nDisallow: /\n";
    let with_unknown =
        "User-agent: FooBot\nInvalid-Unknown-Line: unknown\nUser-agent: *\nDisallow: /\n";
    run(&[
        (
            "sitemap keeps the group open (foo)",
            with_sitemap,
            "FooBot",
            "http://foo.bar/",
            false,
        ),
        (
            "sitemap keeps the group open (bar)",
            with_sitemap,
            "BarBot",
            "http://foo.bar/",
            false,
        ),
        (
            "unknown keeps the group open (foo)",
            with_unknown,
            "FooBot",
            "http://foo.bar/",
            false,
        ),
        (
            "unknown keeps the group open (bar)",
            with_unknown,
            "BarBot",
            "http://foo.bar/",
            false,
        ),
    ]);
}

#[test]
fn id_special_characters() {
    let star = "User-agent: FooBot\nDisallow: /foo/bar/quz\nAllow: /foo/*/qux\n";
    let dollar = "User-agent: FooBot\nDisallow: /foo/bar$\nAllow: /foo/bar/qux\n";
    let hash = "User-agent: FooBot\n# Disallow: /\nDisallow: /foo/quz#qux\nAllow: /\n";
    run(&[
        (
            "star literal path",
            star,
            "FooBot",
            "http://foo.bar/foo/bar/quz",
            false,
        ),
        (
            "star one segment",
            star,
            "FooBot",
            "http://foo.bar/foo/quz",
            true,
        ),
        (
            "star empty segment",
            star,
            "FooBot",
            "http://foo.bar/foo//quz",
            true,
        ),
        (
            "star other segment",
            star,
            "FooBot",
            "http://foo.bar/foo/bax/quz",
            true,
        ),
        (
            "dollar exact",
            dollar,
            "FooBot",
            "http://foo.bar/foo/bar",
            false,
        ),
        (
            "dollar not a prefix",
            dollar,
            "FooBot",
            "http://foo.bar/foo/bar/qux",
            true,
        ),
        (
            "dollar trailing slash",
            dollar,
            "FooBot",
            "http://foo.bar/foo/bar/",
            true,
        ),
        (
            "dollar longer path",
            dollar,
            "FooBot",
            "http://foo.bar/foo/bar/baz",
            true,
        ),
        (
            "comment stripped from value",
            hash,
            "FooBot",
            "http://foo.bar/foo/quz",
            false,
        ),
        (
            "commented rule ignored",
            hash,
            "FooBot",
            "http://foo.bar/foo/bar",
            true,
        ),
    ]);
}

#[test]
fn id_documentation_checks_fish() {
    let fish = "user-agent: FooBot\ndisallow: /\nallow: /fish\n";
    let fish_star = "user-agent: FooBot\ndisallow: /\nallow: /fish*\n";
    for (name, robots) in [("/fish", fish), ("/fish*", fish_star)] {
        run(&[
            (name, robots, "FooBot", "http://foo.bar/bar", false),
            (name, robots, "FooBot", "http://foo.bar/fish", true),
            (name, robots, "FooBot", "http://foo.bar/fish.html", true),
            (
                name,
                robots,
                "FooBot",
                "http://foo.bar/fish/salmon.html",
                true,
            ),
            (name, robots, "FooBot", "http://foo.bar/fishheads", true),
            (
                name,
                robots,
                "FooBot",
                "http://foo.bar/fishheads/yummy.html",
                true,
            ),
            (
                name,
                robots,
                "FooBot",
                "http://foo.bar/fish.html?id=anything",
                true,
            ),
            (name, robots, "FooBot", "http://foo.bar/Fish.asp", false),
            (name, robots, "FooBot", "http://foo.bar/catfish", false),
            (name, robots, "FooBot", "http://foo.bar/?id=fish", false),
            (name, robots, "FooBot", "http://foo.bar/desert/fish", false),
        ]);
    }
}

#[test]
fn id_documentation_checks_fish_directory() {
    let robots = "user-agent: FooBot\ndisallow: /\nallow: /fish/\n";
    let name = "/fish/";
    run(&[
        (name, robots, "FooBot", "http://foo.bar/bar", false),
        (name, robots, "FooBot", "http://foo.bar/fish/", true),
        (name, robots, "FooBot", "http://foo.bar/fish/salmon", true),
        (name, robots, "FooBot", "http://foo.bar/fish/?salmon", true),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/fish/salmon.html",
            true,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/fish/?id=anything",
            true,
        ),
        (name, robots, "FooBot", "http://foo.bar/fish", false),
        (name, robots, "FooBot", "http://foo.bar/fish.html", false),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/Fish/Salmon.html",
            false,
        ),
    ]);
}

#[test]
fn id_documentation_checks_php() {
    let robots = "user-agent: FooBot\ndisallow: /\nallow: /*.php\n";
    let name = "/*.php";
    run(&[
        (name, robots, "FooBot", "http://foo.bar/bar", false),
        (name, robots, "FooBot", "http://foo.bar/filename.php", true),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/folder/filename.php",
            true,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/folder/filename.php?parameters",
            true,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar//folder/any.php.file.html",
            true,
        ),
        (name, robots, "FooBot", "http://foo.bar/filename.php/", true),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/index?f=filename.php/",
            true,
        ),
        (name, robots, "FooBot", "http://foo.bar/php/", false),
        (name, robots, "FooBot", "http://foo.bar/index?php", false),
        (name, robots, "FooBot", "http://foo.bar/windows.PHP", false),
    ]);
}

#[test]
fn id_documentation_checks_php_anchored() {
    let robots = "user-agent: FooBot\ndisallow: /\nallow: /*.php$\n";
    let name = "/*.php$";
    run(&[
        (name, robots, "FooBot", "http://foo.bar/bar", false),
        (name, robots, "FooBot", "http://foo.bar/filename.php", true),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/folder/filename.php",
            true,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/filename.php?parameters",
            false,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/filename.php/",
            false,
        ),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/filename.php5",
            false,
        ),
        (name, robots, "FooBot", "http://foo.bar/php/", false),
        (name, robots, "FooBot", "http://foo.bar/filename?php", false),
        (name, robots, "FooBot", "http://foo.bar/aaaphpaaa", false),
        (name, robots, "FooBot", "http://foo.bar//windows.PHP", false),
    ]);
}

#[test]
fn id_documentation_checks_fish_php() {
    let robots = "user-agent: FooBot\ndisallow: /\nallow: /fish*.php\n";
    let name = "/fish*.php";
    run(&[
        (name, robots, "FooBot", "http://foo.bar/bar", false),
        (name, robots, "FooBot", "http://foo.bar/fish.php", true),
        (
            name,
            robots,
            "FooBot",
            "http://foo.bar/fishheads/catfish.php?parameters",
            true,
        ),
        (name, robots, "FooBot", "http://foo.bar/Fish.PHP", false),
    ]);
}

#[test]
fn id_documentation_checks_precedence() {
    run(&[
        (
            "allow /p beats disallow /",
            "user-agent: FooBot\nallow: /p\ndisallow: /\n",
            "FooBot",
            "http://example.com/page",
            true,
        ),
        (
            "equal length ties to allow",
            "user-agent: FooBot\nallow: /folder\ndisallow: /folder\n",
            "FooBot",
            "http://example.com/folder/page",
            true,
        ),
        (
            "longer wildcard disallow wins",
            "user-agent: FooBot\nallow: /page\ndisallow: /*.htm\n",
            "FooBot",
            "http://example.com/page.htm",
            false,
        ),
        (
            "anchored allow of the root",
            "user-agent: FooBot\nallow: /$\ndisallow: /\n",
            "FooBot",
            "http://example.com/",
            true,
        ),
        (
            "anchored allow does not cover pages",
            "user-agent: FooBot\nallow: /$\ndisallow: /\n",
            "FooBot",
            "http://example.com/page.html",
            false,
        ),
    ]);
}

#[test]
fn google_only_accept_user_agent_up_to_first_space() {
    let robots = "User-Agent: *\nDisallow: /\nUser-Agent: Foo Bar\nAllow: /x/\nDisallow: /\n";
    run(&[
        (
            "token before space matches",
            robots,
            "Foo",
            "http://foo.bar/x/y",
            true,
        ),
        (
            "token before space disallows",
            robots,
            "Foo",
            "http://foo.bar/a/b",
            false,
        ),
    ]);
}

#[test]
fn google_only_typos_in_disallow_are_accepted() {
    for typo in ["dissallow", "dissalow", "disalow", "diasllow", "disallaw"] {
        let robots = format!("user-agent: FooBot\n{typo}: /\n");
        assert!(
            !allowed(&robots, "FooBot", "http://foo.bar/x"),
            "{typo} must be treated as Disallow"
        );
    }
}

#[test]
fn id_utf8_byte_order_mark() {
    let mut robots = vec![0xEF, 0xBB, 0xBF];
    robots.extend_from_slice(b"User-Agent: foo\nDisallow: /AnyValue\n");
    let parsed = RobotsTxt::parse(&robots);
    assert!(!parsed.is_allowed("foo", "http://foo.bar/AnyValue"));
    assert!(parsed.is_allowed("foo", "http://foo.bar/other"));

    // A truncated BOM leaves bytes glued to the key, which is then unknown.
    let mut broken = vec![0xEF, 0xBB];
    broken.extend_from_slice(b"User-Agent: foo\nDisallow: /AnyValue\n");
    assert!(RobotsTxt::parse(&broken).is_allowed("foo", "http://foo.bar/AnyValue"));
}

#[test]
fn crlf_and_cr_line_endings() {
    let robots = "user-agent: FooBot\r\ndisallow: /x/\r\n";
    assert!(!allowed(robots, "FooBot", "http://foo.bar/x/y"));
    let robots = "user-agent: FooBot\rdisallow: /x/\r";
    assert!(!allowed(robots, "FooBot", "http://foo.bar/x/y"));
}

#[test]
fn percent_escapes_are_normalised_in_patterns() {
    let robots = "user-agent: FooBot\ndisallow: /a%2fb\n";
    assert!(!allowed(robots, "FooBot", "http://foo.bar/a%2Fb"));
    let robots = "user-agent: FooBot\ndisallow: /a%2Fb\n";
    assert!(!allowed(robots, "FooBot", "http://foo.bar/a%2Fb"));
}

#[test]
fn plain_paths_and_relative_targets_work() {
    assert!(!allowed(SIMPLE, "FooBot", "/x/y"));
    assert!(!allowed(SIMPLE, "FooBot", "http://foo.bar/x/y"));
    assert!(!allowed(SIMPLE, "FooBot", "/"));
}

#[test]
fn sitemaps_are_collected_globally() {
    let robots = RobotsTxt::parse(
        b"Sitemap: https://foo.bar/sitemap.xml\nuser-agent: *\ndisallow: /\nSitemap: https://foo.bar/news.xml\nSite-map: https://foo.bar/extra.xml\n",
    );
    assert_eq!(
        robots.sitemaps(),
        [
            "https://foo.bar/sitemap.xml",
            "https://foo.bar/news.xml",
            "https://foo.bar/extra.xml"
        ]
    );
}

#[test]
fn crawl_delay_is_read_per_group() {
    let robots = RobotsTxt::parse(
        b"user-agent: *\ncrawl-delay: 10\ndisallow: /a\nuser-agent: FooBot\nCrawl-Delay: 2.5\ndisallow: /b\n",
    );
    assert_eq!(
        robots.crawl_delay("FooBot"),
        Some(Duration::from_millis(2500))
    );
    assert_eq!(robots.crawl_delay("BarBot"), Some(Duration::from_secs(10)));

    let no_delay = RobotsTxt::parse(b"user-agent: *\ndisallow: /\n");
    assert_eq!(no_delay.crawl_delay("FooBot"), None);

    let junk = RobotsTxt::parse(b"user-agent: *\ncrawl-delay: soon\n");
    assert_eq!(junk.crawl_delay("FooBot"), None);

    let negative = RobotsTxt::parse(b"user-agent: *\ncrawl-delay: -5\n");
    assert_eq!(negative.crawl_delay("FooBot"), None);

    let huge = RobotsTxt::parse(b"user-agent: *\ncrawl-delay: 1e9\n");
    assert_eq!(
        huge.crawl_delay("FooBot"),
        Some(Duration::from_secs(86_400))
    );
}

#[test]
fn empty_and_whitespace_files_allow_everything() {
    assert!(RobotsTxt::parse(b"").is_empty());
    assert!(allowed("", "FooBot", "http://foo.bar/anything"));
    assert!(allowed(
        "\n\n   \n# only a comment\n",
        "FooBot",
        "http://foo.bar/anything"
    ));
}

#[test]
fn status_codes_follow_rfc_9309() {
    let body = b"user-agent: *\ndisallow: /\n";
    assert!(!RobotsStatus::from_response(200, body).is_allowed("FooBot", "/x"));
    assert!(RobotsStatus::from_response(404, body).is_allowed("FooBot", "/x"));
    assert!(RobotsStatus::from_response(403, body).is_allowed("FooBot", "/x"));
    assert!(RobotsStatus::from_response(301, body).is_allowed("FooBot", "/x"));
    assert!(!RobotsStatus::from_response(500, body).is_allowed("FooBot", "/x"));
    assert!(!RobotsStatus::from_response(503, body).is_allowed("FooBot", "/x"));
    assert!(!RobotsStatus::from_transport_failure().is_allowed("FooBot", "/x"));
    assert!(RobotsStatus::from_response(204, b"").is_allowed("FooBot", "/x"));
}

#[test]
fn origin_and_robots_url() {
    let url = Url::parse("https://user:pass@foo.bar:8443/deep/path?q=1#f").unwrap();
    assert_eq!(origin_of(&url), "https://foo.bar:8443");
    assert_eq!(
        robots_url(&url).unwrap().as_str(),
        "https://foo.bar:8443/robots.txt"
    );
    let plain = Url::parse("http://foo.bar/x").unwrap();
    assert_eq!(origin_of(&plain), "http://foo.bar");
    assert!(robots_url(&Url::parse("data:text/plain,x").unwrap()).is_none());
}

#[test]
fn cache_honours_the_ttl() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let cache = RobotsCache::default();
    assert_eq!(cache.ttl(), DEFAULT_TTL);
    let url = Url::parse("https://foo.bar/x/y").unwrap();
    assert_eq!(cache.is_allowed(&url, "FooBot", now), None);

    cache.insert(
        origin_of(&url),
        RobotsStatus::from_response(200, b"user-agent: *\ndisallow: /x/\n"),
        now,
    );
    assert_eq!(cache.is_allowed(&url, "FooBot", now), Some(false));
    assert_eq!(
        cache.is_allowed(&Url::parse("https://foo.bar/y").unwrap(), "FooBot", now),
        Some(true)
    );
    assert_eq!(cache.len(), 1);

    let later = now + DEFAULT_TTL - Duration::from_secs(1);
    assert_eq!(cache.is_allowed(&url, "FooBot", later), Some(false));
    let expired = now + DEFAULT_TTL;
    assert_eq!(cache.is_allowed(&url, "FooBot", expired), None);
    assert_eq!(cache.purge_expired(expired), 1);
    assert!(cache.is_empty());
}

#[test]
fn cache_is_keyed_by_origin() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let cache = RobotsCache::default();
    let https = Url::parse("https://foo.bar/x").unwrap();
    let http = Url::parse("http://foo.bar/x").unwrap();
    cache.insert(
        origin_of(&https),
        RobotsStatus::from_response(200, b"user-agent: *\ndisallow: /\n"),
        now,
    );
    assert_eq!(cache.is_allowed(&https, "FooBot", now), Some(false));
    assert_eq!(cache.is_allowed(&http, "FooBot", now), None);
    cache.clear();
    assert!(cache.is_empty());
}
