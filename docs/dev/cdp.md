# Running the CDP host

`tentabrowser cdp` serves the Chrome DevTools Protocol so Puppeteer and
Playwright can drive the engine (`docs/spec/09-cdp-compat.md`).

## Run it

```bash
cargo build -p tentabrowser-cli --features v8,render
target/debug/tentabrowser --ephemeral cdp --port 9222
# DevTools listening on ws://127.0.0.1:9222/devtools/browser/<id>
```

Connect with `puppeteer.connect({ browserWSEndpoint })` using that line.
Without `v8` pages run no script, without `render` screenshots, layout
metrics and geometry answer an error naming the feature.

- `--host`, `--port` (0 picks a free port), `--max-targets`.
- `--token` or the environment variable `TENTA_CDP_TOKEN`: every request must
  carry `?token=` (connect with the full endpoint). Required, 16 bytes or more,
  to listen on a non-loopback address. Prefer the variable: a flag shows in the
  process list.
- `--allowed-origin ORIGIN` (repeatable): admit requests with that exact
  `Origin`. Without any, web pages cannot connect.
- Global flags apply to the default context: `--allow-private-network` (needed
  for loopback sites), `--proxy`, `--user-agent`, `--accept-language`.

There is no authentication on loopback by default: any local process can drive
the browser. Set a token on shared machines.

## Domains

`Browser`, `Target` (including Chromium's `tab` wrapper targets, which
Puppeteer 22.6 and later attach to), `Inspector`, `Storage` (cookies), `Page`,
`Runtime`, `DOM`, `Input`, `Emulation`, `Performance`, `Log`, `Security`.
`Network` and `Audits` accept `enable`/`disable` and send no events; the rest
of `Network` and `Fetch` are not built. `/json/protocol` lists exactly what is
implemented; anything else is `-32601`. Details per package are in
`docs/dev/p3-plan.md` (M12).

## Limitations a client will meet

- Input is synthetic (`isTrusted` is false). A click on a plain link or a GET
  form submission navigates the page; POST forms and script-set `location`
  do not (the engine has no `location`), so `waitForNavigation` around them
  waits until its timeout.
- Only `http(s)` link and form targets are followed; `file:` and `javascript:`
  do nothing and leave the page as it is. GET forms have no validation and
  encode as UTF-8.
- Typing into `contenteditable` answers an error. Text inputs and textareas
  are edited; form control values are not painted in screenshots yet.
- No scrolling, touch, drag or file upload (`DOM.setFileInputFiles` is
  refused by name).
- `Emulation`: viewport and scale factor only; the user agent is fixed per
  browser context, and the other commands are refused by name.
- `page.goto` resolves `null` because no `Network` events are sent.
- `crates/tenta-cdp/src/interaction.js` gives every CDP-created page focus,
  stub `IntersectionObserver`, a polling `MutationObserver` and geometry
  members until the native ones exist. It lives in the page's trust domain,
  is visible to a page that looks for it, and is not a stealth layer.

## Smoke test

`tests/cdp-clients/smoke/` holds a stock Puppeteer script and its fixture page;
its header lists the steps to run it.
