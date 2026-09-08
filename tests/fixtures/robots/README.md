# `robots/`

Three pages behind the repository-root `tests/fixtures/robots.txt`, which
disallows `/robots/private/` for every user agent.

| Path | Robots verdict | Marker |
|------|----------------|--------|
| `/robots/index.html` | allowed; links to both other pages | `ROBOTS-INDEX-OK` |
| `/robots/allowed.html` | allowed | `ROBOTS-ALLOWED-OK` |
| `/robots/private/secret.html` | disallowed | `ROBOTS-SECRET-LEAKED` |

`tentabrowser scrape` must print `{"url": …, "error": "robots"}` for the
third URL and never request it; the marker's name is what a test greps for to
prove no fetch happened. `--ignore-robots` fetches all three.
