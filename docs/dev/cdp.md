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
`Runtime`, `DOM`, `Input`, `Emulation`, `Performance`, `Log`, `Security`,
`Network` and `Fetch`. `Audits` accepts `enable`/`disable` and sends nothing.
`/json/protocol` lists exactly what is implemented; anything else is `-32601`.
Details per package are in `docs/dev/p3-plan.md` (M12).

### Network and Fetch

`Network.enable` reports every request of the page: the navigation with its
redirect chain (`redirectResponse`, one `requestId` that equals the loader id,
as Puppeteer expects), scripts, images, stylesheets, `fetch()` and XHR, each
with `requestWillBeSent`, `responseReceived`, `dataReceived` and
`loadingFinished` or `loadingFailed` (`net::ERR_*`, `canceled`,
`blockedReason`). Also built: `getResponseBody` (kept per page in a 32 MiB LRU;
a body over 16 MiB is not kept), `getRequestPostData`, `setExtraHTTPHeaders`,
`setUserAgentOverride` (also `Emulation.setUserAgentOverride`),
`setCacheDisabled`, `clearBrowserCache`, `emulateNetworkConditions` (only
`offline` has an effect), `getCookies`, `getAllCookies`, `setCookie(ies)`,
`deleteCookies` and `clearBrowserCookies`, all over the context's jar.
Overrides belong to the session that set them and end with it.

`Fetch.enable` holds the page's requests that match `patterns` (`urlPattern`
wildcards, `resourceType`) at the request stage, before the cache, including
each redirect hop. `continueRequest` (url, method, postData, headers),
`fulfillRequest` and `failRequest` release them; `networkId` pairs a paused
request with its `Network` events. One session at a time may hold a page's
requests, and a session that leaves releases everything it held. The response
stage, `continueWithAuth` (the engine raises no auth challenges) and
`getResponseBody` of `Fetch` are refused by name.

A request held by `Fetch` waits for its client the way Chromium's does: there
is no timeout of its own, so a client that never answers stalls that request
until the page's navigation deadline, a newer navigation or closing the page
ends it. `Fetch.disable`, the holding session detaching, or the connection
closing releases every request it held, unchanged. A response a client
fulfills is a response like any other: a 3xx with `Location` is followed (and
held again) and `Set-Cookie` is stored. `continueRequest` with another `url`
meets the destination policy again. Headers that cannot go on the wire are an
invalid-params error. `Network.setCookie` answers `success: false`, and
`setCookies` an error, for `SameSite=None` without `Secure`, `__Secure-` and
`__Host-` cookies that break their prefix rules and a `domain` that does not
cover the URL's host; `deleteCookies` matches `domain` exactly, so
`example.com` names a host-only cookie and `.example.com` a domain cookie.
Overrides (`setExtraHTTPHeaders`, user agent, cache, offline) are kept per
setting and per session: a session that leaves undoes only its own.
A session's event queue merges consecutive `dataReceived` events of a request
and sheds the oldest of them past 8192 queued events; nothing else is dropped.
A session that enables the domain mid-flight hears nothing of requests that
began before, redirects included.

What a client will not find: the transport's own headers (profile `User-Agent`,
`Accept`, `Cookie`) are not in `request.headers`; `encodedDataLength` counts
decoded bytes; a redirect response carries its headers and status only;
`requestWillBeSentExtraInfo`, `requestServedFromCache`, WebSocket and
EventSource events are not sent; `data:` URLs (an image, `fetch('data:...')`)
are handled without the transport and emit no events, where Chromium reports
them; the user agent override changes the header,
not `navigator.userAgent`; stylesheets are requested when the page is laid out
(screenshot, geometry), not while it loads, so no `Stylesheet` request appears
before the first render.

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
- `Emulation`: viewport, scale factor and the user agent header; the other
  commands are refused by name.
- `crates/tenta-cdp/src/interaction.js` gives every CDP-created page a stub
  `IntersectionObserver` (it reads the native rectangles once per target) and the
  pointer and keyboard default actions until the native ones exist. Focus,
  `MutationObserver` and the geometry members (`getBoundingClientRect`,
  `offsetWidth`, `getComputedStyle`, `elementFromPoint`, ...) are native; the
  script only drives them. It lives in the page's trust
  domain, is visible to a page that looks for it, and is not a stealth layer.

## Smoke test

`tests/cdp-clients/smoke/` holds a stock Puppeteer script and its fixture page;
its header lists the steps to run it.
