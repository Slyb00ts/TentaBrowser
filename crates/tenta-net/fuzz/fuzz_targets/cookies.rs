#![no_main]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use libfuzzer_sys::fuzz_target;
use tenta_net::policy::cookies::{parse_cookie_date, parse_set_cookie};
use url::Url;

// A `Set-Cookie` value is attacker-controlled bytes. Parsing must never
// panic and must be deterministic; the cookie date parser gets the same
// bytes as text.
fuzz_target!(|data: &[u8]| {
    let request = Url::parse("https://sub.example.com/a/b?q=1").expect("static url");
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let first = parse_set_cookie(data, &request, now);
    let second = parse_set_cookie(data, &request, now);
    assert_eq!(first, second, "cookie parsing is not deterministic");

    let text = String::from_utf8_lossy(data);
    let date = parse_cookie_date(&text);
    assert_eq!(date, parse_cookie_date(&text));
    if let Some(when) = date {
        // Dates parse to something the clock can represent.
        let _ = when.duration_since(SystemTime::UNIX_EPOCH);
    }
});
