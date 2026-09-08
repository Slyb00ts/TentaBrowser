# CDP method checklist

The list of Chrome DevTools Protocol methods and events that Puppeteer and
Playwright actually use, derived from their sources as `09-cdp-compat.md` §2
requires. It is the input for `tenta-cdp`'s dispatch table and for the
per-method status tracking that starts in P4.

- Checklist: `crates/tenta-cdp/checklist/methods.toml` (generated).
- Generator: `crates/tenta-cdp/checklist/extract.py` (Python 3, stdlib only).
- Conformance suites (spec `15-testing-and-benchmarks.md` §5): section
  "Conformance suites" below.

## Pinned client revisions

| Client | Repository | Commit | Commit date | Package version |
|---|---|---|---|---|
| Puppeteer | `https://github.com/puppeteer/puppeteer` | `a908d92d1541d4fbc5506fcd972f8872e38d12de` | 2026-09-03 | `puppeteer-core` 25.10.0 |
| Playwright | `https://github.com/microsoft/playwright` | `46cd5008d12d4e1297793d921e6cc3b595e388da` | 2026-09-04 | `playwright-core` 1.64.0-next |

Derived on 2026-09-06 from the default branches (`main`) at those commits.

## How to regenerate

```bash
SCRATCH=$(mktemp -d)
git clone --depth 1 --filter=blob:none --sparse https://github.com/puppeteer/puppeteer "$SCRATCH/puppeteer"
git -C "$SCRATCH/puppeteer" sparse-checkout set packages/puppeteer-core/src test/src
git clone --depth 1 --filter=blob:none --sparse https://github.com/microsoft/playwright "$SCRATCH/playwright"
git -C "$SCRATCH/playwright" sparse-checkout set packages/playwright-core/src tests/page tests/library
git -C "$SCRATCH/puppeteer" rev-parse HEAD    # update the table above
git -C "$SCRATCH/playwright" rev-parse HEAD
python3 crates/tenta-cdp/checklist/extract.py "$SCRATCH/puppeteer" "$SCRATCH/playwright" --unclassified
```

To pin a specific commit instead of the branch head, add
`git -C <clone> fetch --depth 1 origin <sha> && git -C <clone> checkout <sha>`
after the clone. `--unclassified` prints every `'Domain.name'` literal the
classifier could not attribute to a send or a listener; the expected output at
the pinned commits is one line (`Network.emulateNetworkConditions` in
Puppeteer's `Connection.ts`, a comparison inside `send()`, already counted
from its real call sites).

### What the generator scans

| Client | Files | Call shapes recognised |
|---|---|---|
| Puppeteer | `packages/puppeteer-core/src/cdp/**/*.ts` (not `*.test.ts`), plus `src/api/Page.ts`, `src/node/ScreenRecorder.ts` and `src/common/util.ts`, the only files outside `cdp/` that send raw CDP (`Page.startScreencast`/`stopScreencast`/`screencastFrame(Ack)`, `IO.read`/`IO.close`) | `client.send('X.y'`, `send<T>('X.y'`, `.on/.once/.off('X.y'`, `emit('X.y'`, `fromEmitterEvent(client, 'X.y'`, `object.method === 'X.y'`, the `[['X.y', handler], ...]` table in `NetworkManager.ts` |
| Playwright | `packages/playwright-core/src/server/chromium/**/*.ts` (not `protocol.d.ts`) | `session.send('X.y'`, `_client.send(`, `_session.send(`, `_send(`, `_sendMayFail(`, `{ method: 'X.y' }`, `eventsHelper.addEventListener(session, 'X.y'`, `session.on/.once/.off('X.y'` |

Excluded on purpose:

- Puppeteer `src/cdp/ExtensionTransport.ts`: it emulates a CDP server on
  top of `chrome.debugger` for the extension build (`case 'Target.setAutoAttach'`
  handlers, synthesised `Target.targetCreated` messages). Nothing in it is
  sent to a browser.
- Puppeteer `src/bidi/**`: the WebDriver BiDi backend. It still sends ten CDP
  methods through the BiDi `goog:cdp` mapper (`Autofill.trigger`,
  `Browser.close`, `Browser.getVersion`, `DOM.describeNode`, `DOM.resolveNode`,
  `Network.emulateNetworkConditions`, `Network.setCacheDisabled`,
  `Page.setBypassCSP`, `Runtime.queryObjects`, `Target.detachFromTarget`); all
  ten are already in the checklist from the CDP backend.
- Puppeteer `src/node/ChromeLauncher.ts` and the rest of `src/node/**`,
  `src/api/**`, `src/common/**`: no CDP calls apart from the three files
  listed above (`api/CDPSession.ts` only carries an `Animation.*` doc
  example, which is not counted).
- Comments: a small tokenizer blanks `//` and `/* */` comments while keeping
  string and regex literals intact, so doc comments such as the event list in
  `TargetManager.ts` are not counted.

### `connect_over_cdp`

`connect_over_cdp = true` marks a Playwright use that is reachable after
`chromium.connectOverCDP()`. Rule applied: every file under
`server/chromium/` except `chromium.ts` and `chromiumSwitches.ts` is on that
path, because `CRBrowser.connect()` builds the same `CRBrowser`, `CRPage`,
`CRNetworkManager`, `CRServiceWorker`, coverage and PDF objects for a
launched and for a connected browser. Inside `chromium.ts` a use counts only
when it sits in `connectOverCDP` / `_connectOverCDPInternal` /
`_connectOverCDPImpl`. At the pinned commit that leaves one Playwright entry
without the flag: `Browser.close` from `_attemptToGracefullyCloseBrowser`
(launch path only). The worker-only `connectToWorker()` path in
`chromium.ts` (Node.js inspector targets) contributes `Runtime.*` names that
are also used elsewhere.

## Totals per domain

Names are unique per (name, kind). "Puppeteer only" / "Playwright only" /
"Both" count names, not call sites.

| Domain | In spec §2 | Methods | Events | Puppeteer only | Playwright only | Both |
|---|---|---|---|---|---|---|
| Accessibility | yes | 2 | 0 | 2 | 0 | 0 |
| Audits | **no** | 1 | 1 | 2 | 0 | 0 |
| Autofill | **no** | 1 | 0 | 1 | 0 | 0 |
| BluetoothEmulation | **no** | 3 | 0 | 3 | 0 | 0 |
| Browser | yes | 12 | 2 | 2 | 4 | 8 |
| CSS | yes | 5 | 1 | 0 | 0 | 6 |
| DOM | yes | 9 | 0 | 0 | 2 | 7 |
| Debugger | yes | 5 | 2 | 0 | 2 | 5 |
| DeviceAccess | **no** | 3 | 1 | 4 | 0 | 0 |
| Emulation | yes | 18 | 0 | 8 | 1 | 9 |
| Extensions | **no** | 4 | 0 | 4 | 0 | 0 |
| Fetch | yes | 6 | 2 | 0 | 0 | 8 |
| HeapProfiler | **no** | 4 | 1 | 4 | 0 | 1 |
| IO | yes | 2 | 0 | 0 | 0 | 2 |
| Input | yes | 6 | 1 | 0 | 0 | 7 |
| Inspector | **no** | 0 | 3 | 0 | 1 | 2 |
| Log | yes | 1 | 1 | 0 | 0 | 2 |
| Network | yes | 14 | 14 | 7 | 9 | 12 |
| PWA | **no** | 5 | 0 | 5 | 0 | 0 |
| Page | yes | 25 | 15 | 7 | 4 | 29 |
| Performance | yes | 2 | 1 | 3 | 0 | 0 |
| Profiler | yes | 5 | 0 | 0 | 0 | 5 |
| Runtime | yes | 10 | 6 | 2 | 1 | 13 |
| Security | yes | 1 | 0 | 0 | 0 | 1 |
| Storage | yes | 3 | 0 | 0 | 1 | 2 |
| Target | yes | 13 | 5 | 7 | 2 | 9 |
| Tracing | yes | 2 | 1 | 0 | 0 | 3 |
| WebMCP | **no** | 3 | 4 | 7 | 0 | 0 |
| **Total** | | 165 | 61 | 68 | 27 | 131 |

- Methods: 165, events: 61, entries: 226.
- Playwright uses 158 names, 157 of them on the `connectOverCDP` path.
- Puppeteer uses 199 names.

### Full list by domain

- **Accessibility**: `getFullAXTree` [puppeteer], `queryAXTree` [puppeteer]
- **Audits**: `enable` [puppeteer], `issueAdded` (event) [puppeteer]
- **Autofill**: `trigger` [puppeteer]
- **BluetoothEmulation**: `disable` [puppeteer], `enable` [puppeteer], `simulatePreconnectedPeripheral` [puppeteer]
- **Browser**: `cancelDownload` [playwright], `close` [both], `downloadProgress` (event) [playwright], `downloadWillBegin` (event) [playwright], `getVersion` [both], `getWindowBounds` [both], `getWindowForTarget` [both], `grantPermissions` [both], `resetPermissions` [both], `setContentsSize` [puppeteer], `setDockTile` [playwright], `setDownloadBehavior` [both], `setPermission` [puppeteer], `setWindowBounds` [both]
- **CSS**: `disable` [both], `enable` [both], `getStyleSheetText` [both], `startRuleUsageTracking` [both], `stopRuleUsageTracking` [both], `styleSheetAdded` (event) [both]
- **DOM**: `describeNode` [both], `disable` [both], `enable` [both], `getBoxModel` [playwright], `getContentQuads` [playwright], `getFrameOwner` [both], `resolveNode` [both], `scrollIntoViewIfNeeded` [both], `setFileInputFiles` [both]
- **Debugger**: `disable` [both], `enable` [both], `getScriptSource` [both], `paused` (event) [playwright], `resume` [playwright], `scriptParsed` (event) [both], `setSkipAllPauses` [both]
- **DeviceAccess**: `cancelPrompt` [puppeteer], `deviceRequestPrompted` (event) [puppeteer], `enable` [puppeteer], `selectPrompt` [puppeteer]
- **Emulation**: `addScreen` [puppeteer], `clearDeviceMetricsOverride` [puppeteer], `clearIdleOverride` [puppeteer], `getScreenInfos` [puppeteer], `removeScreen` [puppeteer], `setCPUThrottlingRate` [puppeteer], `setDefaultBackgroundColorOverride` [both], `setDeviceMetricsOverride` [both], `setEmulatedMedia` [both], `setEmulatedVisionDeficiency` [puppeteer], `setFocusEmulationEnabled` [both], `setGeolocationOverride` [both], `setIdleOverride` [puppeteer], `setLocaleOverride` [both], `setScriptExecutionDisabled` [both], `setTimezoneOverride` [both], `setTouchEmulationEnabled` [both], `setUserAgentOverride` [playwright]
- **Extensions**: `getExtensions` [puppeteer], `loadUnpacked` [puppeteer], `triggerAction` [puppeteer], `uninstall` [puppeteer]
- **Fetch**: `authRequired` (event) [both], `continueRequest` [both], `continueWithAuth` [both], `disable` [both], `enable` [both], `failRequest` [both], `fulfillRequest` [both], `requestPaused` (event) [both]
- **HeapProfiler**: `addHeapSnapshotChunk` (event) [puppeteer], `collectGarbage` [both], `disable` [puppeteer], `enable` [puppeteer], `takeHeapSnapshot` [puppeteer]
- **IO**: `close` [both], `read` [both]
- **Input**: `dispatchDragEvent` [both], `dispatchKeyEvent` [both], `dispatchMouseEvent` [both], `dispatchTouchEvent` [both], `dragIntercepted` (event) [both], `insertText` [both], `setInterceptDrags` [both]
- **Inspector**: `targetCrashed` (event) [both], `targetReloadedAfterCrash` (event) [playwright], `workerScriptLoaded` (event) [both]
- **Log**: `enable` [both], `entryAdded` (event) [both]
- **Network**: `clearBrowserCache` [playwright], `deleteCookies` [puppeteer], `emulateNetworkConditions` [both], `emulateNetworkConditionsByRule` [puppeteer], `enable` [both], `getCookies` [puppeteer], `getRequestPostData` [puppeteer], `getResponseBody` [both], `loadNetworkResource` [playwright], `loadingFailed` (event) [both], `loadingFinished` (event) [both], `requestServedFromCache` (event) [both], `requestWillBeSent` (event) [both], `requestWillBeSentExtraInfo` (event) [both], `responseReceived` (event) [both], `responseReceivedExtraInfo` (event) [both], `setBypassServiceWorker` [puppeteer], `setCacheDisabled` [both], `setCookies` [puppeteer], `setExtraHTTPHeaders` [both], `setUserAgentOverride` [puppeteer], `webSocketClosed` (event) [playwright], `webSocketCreated` (event) [playwright], `webSocketFrameError` (event) [playwright], `webSocketFrameReceived` (event) [playwright], `webSocketFrameSent` (event) [playwright], `webSocketHandshakeResponseReceived` (event) [playwright], `webSocketWillSendHandshakeRequest` (event) [playwright]
- **PWA**: `changeAppUserSettings` [puppeteer], `getOsAppState` [puppeteer], `install` [puppeteer], `launch` [puppeteer], `uninstall` [puppeteer]
- **Page**: `addScriptToEvaluateOnNewDocument` [both], `bringToFront` [both], `captureScreenshot` [both], `close` [both], `createIsolatedWorld` [both], `domContentEventFired` (event) [puppeteer], `enable` [both], `fileChooserOpened` (event) [both], `frameAttached` (event) [both], `frameDetached` (event) [both], `frameNavigated` (event) [both], `frameRequestedNavigation` (event) [playwright], `frameStartedLoading` (event) [puppeteer], `frameStoppedLoading` (event) [puppeteer], `getFrameTree` [both], `getLayoutMetrics` [playwright], `getNavigationHistory` [both], `handleJavaScriptDialog` [both], `javascriptDialogClosed` (event) [both], `javascriptDialogOpening` (event) [both], `lifecycleEvent` (event) [both], `loadEventFired` (event) [puppeteer], `navigate` [both], `navigateToHistoryEntry` [both], `navigatedWithinDocument` (event) [both], `printToPDF` [both], `reload` [both], `removeScriptToEvaluateOnNewDocument` [both], `screencastFrame` (event) [both], `screencastFrameAck` [both], `setBypassCSP` [both], `setDocumentContent` [puppeteer], `setFontFamilies` [playwright], `setInterceptFileChooserDialog` [both], `setLifecycleEventsEnabled` [both], `startScreenRecording` [puppeteer], `startScreencast` [both], `stopScreenRecording` [puppeteer], `stopScreencast` [both], `windowOpen` (event) [playwright]
- **Performance**: `enable` [puppeteer], `getMetrics` [puppeteer], `metrics` (event) [puppeteer]
- **Profiler**: `disable` [both], `enable` [both], `startPreciseCoverage` [both], `stopPreciseCoverage` [both], `takePreciseCoverage` [both]
- **Runtime**: `addBinding` [both], `bindingCalled` (event) [both], `callFunctionOn` [both], `consoleAPICalled` (event) [both], `disable` [playwright], `enable` [both], `evaluate` [both], `exceptionThrown` (event) [both], `executionContextCreated` (event) [both], `executionContextDestroyed` (event) [both], `executionContextsCleared` (event) [both], `getProperties` [both], `queryObjects` [puppeteer], `releaseObject` [both], `removeBinding` [puppeteer], `runIfWaitingForDebugger` [both]
- **Security**: `setIgnoreCertificateErrors` [both]
- **Storage**: `clearCookies` [playwright], `getCookies` [both], `setCookies` [both]
- **Target**: `attachToBrowserTarget` [playwright], `attachToTarget` [both], `attachedToTarget` (event) [both], `closeTarget` [both], `createBrowserContext` [both], `createTarget` [both], `detachFromTarget` [both], `detachedFromTarget` (event) [both], `disposeBrowserContext` [both], `getBrowserContexts` [puppeteer], `getDevToolsTarget` [puppeteer], `getTargetInfo` [playwright], `openDevTools` [puppeteer], `setAutoAttach` [both], `setDiscoverTargets` [puppeteer], `targetCreated` (event) [puppeteer], `targetDestroyed` (event) [puppeteer], `targetInfoChanged` (event) [puppeteer]
- **Tracing**: `end` [both], `start` [both], `tracingComplete` (event) [both]
- **WebMCP**: `cancelInvocation` [puppeteer], `enable` [puppeteer], `invokeTool` [puppeteer], `toolInvoked` (event) [puppeteer], `toolResponded` (event) [puppeteer], `toolsAdded` (event) [puppeteer], `toolsRemoved` (event) [puppeteer]

## Cross-check against `09-cdp-compat.md` §2

Findings the spec owner should fold into the domain table (the spec is not
edited here).

### Location of the checklist

§2 says the checklist is "committed as `tenta-cdp/COVERAGE.md` with a status
column". The generated artefact is `crates/tenta-cdp/checklist/methods.toml`
(one `[[entry]]` per name); a status column can be added there as a
`status` key when P4 starts, and `COVERAGE.md` can be rendered from it.

### Domains the clients use that the table does not mention

| Domain | Used by | Names | Suggested handling |
|---|---|---|---|
| `Inspector` | both | `targetCrashed`, `workerScriptLoaded` (both); `targetReloadedAfterCrash` (Playwright) | §3 already lists `Inspector.targetCrashed`; the domain should appear in the §2 table. Neither client listens for `Target.targetCrashed`, so `Inspector.targetCrashed` is the one that matters. `workerScriptLoaded` gates worker attachment in both clients. |
| `HeapProfiler` | both (`collectGarbage`); Puppeteer (`enable`, `disable`, `takeHeapSnapshot`, `addHeapSnapshotChunk`) | 5 | `collectGarbage` is called by Playwright's `page.requestGC()` and Puppeteer; needs at least a no-op or a real GC trigger. Heap snapshots are a §4 non-goal; return `-32601`. |
| `Audits` | Puppeteer | `enable`, `issueAdded` | Puppeteer's `page.on('issue')` (`CdpIssue.ts`). Enable can be a no-op that never emits. |
| `DeviceAccess` | Puppeteer | `enable`, `cancelPrompt`, `selectPrompt`, `deviceRequestPrompted` | WebUSB/WebBluetooth device prompts; no-op unless WebBluetooth lands. |
| `BluetoothEmulation` | Puppeteer | `enable`, `disable`, `simulatePreconnectedPeripheral` | Not in scope; `-32601`. |
| `Autofill` | Puppeteer | `trigger` | `ElementHandle.autofill()`; `-32601` unless forms autofill is specified. |
| `Extensions` | Puppeteer | `getExtensions`, `loadUnpacked`, `triggerAction`, `uninstall` | Chrome extensions are out of scope; `-32601`. |
| `PWA` | Puppeteer | `install`, `launch`, `uninstall`, `getOsAppState`, `changeAppUserSettings` | Out of scope; `-32601`. |
| `WebMCP` | Puppeteer | `enable`, `invokeTool`, `cancelInvocation`, `toolInvoked`, `toolResponded`, `toolsAdded`, `toolsRemoved` | Chrome's WebMCP experiment (`cdp/WebMCP.ts`). Out of scope for now; `-32601`. Worth a note in `10-mcp.md` since it overlaps conceptually with TentaBrowser's MCP server. |

Spec domains that neither client touches over CDP at the pinned commits:
`DOMSnapshot`, `WebAudio`, `Media`. Playwright does not use
`Accessibility` over CDP at all (its ARIA snapshots run in the injected
script); only Puppeteer calls `Accessibility.getFullAXTree` / `queryAXTree`.

### Methods and events in spec domains the notes should mention

- **Browser**: `getWindowForTarget`, `getWindowBounds`, `setWindowBounds`
  (both, viewport and `bringToFront` paths); `setDockTile` (Playwright, macOS
  icon; no-op); `setContentsSize` (Puppeteer, viewport without device metrics
  override); `setPermission` and `resetPermissions` (both/Puppeteer);
  `cancelDownload` plus the events `Browser.downloadWillBegin` and
  `Browser.downloadProgress` (Playwright). §2 lists `downloadWillBegin` and
  `downloadProgress` under **Page**; Playwright subscribes to the **Browser**
  versions after `Browser.setDownloadBehavior { eventsEnabled: true }`, so
  both flavours (or at least the Browser one) are needed.
- **Target**: `attachToBrowserTarget` (Playwright, used on the
  `connectOverCDP` path to obtain the browser session), `getTargetInfo`
  (Playwright), `getDevToolsTarget` / `openDevTools` (Puppeteer,
  `-32601` is fine), `createBrowserContext` / `disposeBrowserContext`,
  `Target.targetCreated` / `targetDestroyed` / `targetInfoChanged` (Puppeteer
  relies on `setDiscoverTargets`; Playwright relies only on `setAutoAttach`).
- **Page**: `getLayoutMetrics` (Playwright screenshots), `setFontFamilies`
  (Playwright sets platform default fonts on every page; must not fail),
  `setLifecycleEventsEnabled`, `removeScriptToEvaluateOnNewDocument`,
  `reload`, `bringToFront`, `close`, `screencastFrameAck`,
  `frameRequestedNavigation` and `windowOpen` events (Playwright),
  `frameStartedLoading` / `frameStoppedLoading` / `domContentEventFired`
  (Puppeteer), `startScreenRecording` / `stopScreenRecording` (Puppeteer's
  `page.screencast()`, experimental and not in `devtools-protocol` yet;
  `-32601` makes that API fail, while `page.on('screencastframe')`-style
  use goes through `Page.startScreencast` and keeps working).
- **Emulation**: `setFocusEmulationEnabled` and
  `setDefaultBackgroundColorOverride` (both, called on every Playwright page),
  `setUserAgentOverride` (Playwright uses the Emulation variant, Puppeteer the
  `Network.setUserAgentOverride` variant; both must work), `setIdleOverride` /
  `clearIdleOverride`, `setCPUThrottlingRate`, `addScreen` / `removeScreen` /
  `getScreenInfos` (Puppeteer, multi-screen emulation, new).
- **Network**: `requestWillBeSentExtraInfo` / `responseReceivedExtraInfo`
  (both clients pair them with the main events; Puppeteer's
  `NetworkEventManager` buffers on them), the seven `webSocket*` events
  (Playwright's `page.on('websocket')`), `emulateNetworkConditionsByRule` and
  `setBypassServiceWorker` (Puppeteer), `loadNetworkResource` and
  `clearBrowserCache` (Playwright), `getRequestPostData` (Puppeteer).
- **Storage**: `getCookies` / `setCookies` / `clearCookies` with
  `browserContextId` (Playwright's cookie API goes through **Storage**, not
  **Network**; Puppeteer uses `Network.getCookies` / `setCookies` /
  `deleteCookies`). §2 lists cookies under Network only.
- **Debugger**: §2 schedules Debugger for P8 as "no source-level debugging",
  but Playwright's coverage (`crCoverage.ts`) needs `Debugger.enable`,
  `setSkipAllPauses`, `scriptParsed`, `getScriptSource`, and handles
  `Debugger.paused` with `resume`; Puppeteer's `Coverage` needs the same
  minus `paused`. **CSS** coverage needs `CSS.startRuleUsageTracking` /
  `stopRuleUsageTracking` / `styleSheetAdded` / `getStyleSheetText` (both).
  These belong with `Profiler.startPreciseCoverage` in the P8 row.
- **Runtime**: `runIfWaitingForDebugger` (both, after every auto-attach;
  must resolve promptly), `executionContextsCleared`, `removeBinding`,
  `queryObjects` (Puppeteer), `getProperties`.
- **DOM**: `getFrameOwner` (both, iframe element lookup), `getBoxModel` /
  `getContentQuads` (Playwright). §2 already lists the rest.
- **Input**: `setInterceptDrags` and the `dragIntercepted` event (both), in
  addition to `dispatchDragEvent`.
- **Performance**: `enable`, `getMetrics`, `metrics` event (Puppeteer
  `page.metrics()`).
- **IO**: `read` / `close` are used by both for PDF and screen recording
  streams.
- **Security**: only `setIgnoreCertificateErrors` (both).
- **Tracing**: `start`, `end`, `tracingComplete` (both) with
  `transferMode: 'ReturnAsStream'` and `IO.read`.

## Conformance suites

Spec `15-testing-and-benchmarks.md` §5 names the files below. "Cases" is a
rough count of `it(` / `test(` (including `.skip`, `.fixme`, `.only`
variants) at the pinned commit, before any `describe`-level skip logic.

### Puppeteer (`test/src/`)

All 13 named files exist at `a908d92d`; 592 cases in total. Puppeteer's
harness additionally applies `test/TestExpectations.json`, which must be
extended with an entry for the TentaBrowser launcher.

| File | Exists at pin | Cases |
|---|---|---|
| `test/src/navigation.test.ts` | yes | 60 |
| `test/src/page.test.ts` | yes | 152 |
| `test/src/evaluation.test.ts` | yes | 52 |
| `test/src/click.test.ts` | yes | 30 |
| `test/src/keyboard.test.ts` | yes | 19 |
| `test/src/mouse.test.ts` | yes | 14 |
| `test/src/network.test.ts` | yes | 56 |
| `test/src/requestinterception.test.ts` | yes | 53 |
| `test/src/screenshot.test.ts` | yes | 30 |
| `test/src/frame.test.ts` | yes | 21 |
| `test/src/target.test.ts` | yes | 20 |
| `test/src/emulation.test.ts` | yes | 26 |
| `test/src/waittask.test.ts` | yes | 59 |

### Playwright `tests/page/*.spec.ts`

134 files at `46cd5008`, 2923 cases. The list includes the `expect-*`,
`locator-*`, `selectors-*` and `page-aria-snapshot*` files, which run
almost entirely in the injected script and exercise Runtime/DOM more than
Page; the dialog tests (`page-dialog.spec.ts`) and network tests
(`page-network-*.spec.ts`, `page-request-*.spec.ts`, `page-route.spec.ts`,
`interception.spec.ts`) are in this directory as the spec says. Six
`*.spec.ts-snapshots` directories and `pageTest.ts` / `pageTestApi.ts` sit
alongside and are not test files.

| File | Cases |
|---|---|
| `tests/page/elementhandle-bounding-box.spec.ts` | 11 |
| `tests/page/elementhandle-click.spec.ts` | 9 |
| `tests/page/elementhandle-content-frame.spec.ts` | 5 |
| `tests/page/elementhandle-convenience.spec.ts` | 15 |
| `tests/page/elementhandle-eval-on-selector.spec.ts` | 6 |
| `tests/page/elementhandle-misc.spec.ts` | 11 |
| `tests/page/elementhandle-owner-frame.spec.ts` | 7 |
| `tests/page/elementhandle-press.spec.ts` | 6 |
| `tests/page/elementhandle-query-selector.spec.ts` | 7 |
| `tests/page/elementhandle-screenshot.spec.ts` | 22 |
| `tests/page/elementhandle-scroll-into-view.spec.ts` | 12 |
| `tests/page/elementhandle-select-text.spec.ts` | 5 |
| `tests/page/elementhandle-type.spec.ts` | 6 |
| `tests/page/elementhandle-wait-for-element-state.spec.ts` | 12 |
| `tests/page/eval-on-selector-all.spec.ts` | 9 |
| `tests/page/eval-on-selector.spec.ts` | 26 |
| `tests/page/expect-boolean.spec.ts` | 97 |
| `tests/page/expect-builtins.spec.ts` | 107 |
| `tests/page/expect-matcher-result.spec.ts` | 6 |
| `tests/page/expect-misc.spec.ts` | 74 |
| `tests/page/expect-timeout.spec.ts` | 15 |
| `tests/page/expect-to-have-accessible.spec.ts` | 18 |
| `tests/page/expect-to-have-text.spec.ts` | 36 |
| `tests/page/expect-to-have-value.spec.ts` | 12 |
| `tests/page/expect-with-snapshot.spec.ts` | 18 |
| `tests/page/frame-evaluate.spec.ts` | 16 |
| `tests/page/frame-frame-element.spec.ts` | 6 |
| `tests/page/frame-goto.spec.ts` | 6 |
| `tests/page/frame-hierarchy.spec.ts` | 17 |
| `tests/page/interception.spec.ts` | 21 |
| `tests/page/jshandle-as-element.spec.ts` | 4 |
| `tests/page/jshandle-evaluate.spec.ts` | 2 |
| `tests/page/jshandle-json-value.spec.ts` | 3 |
| `tests/page/jshandle-properties.spec.ts` | 7 |
| `tests/page/jshandle-to-string.spec.ts` | 7 |
| `tests/page/locator-any-frame.spec.ts` | 50 |
| `tests/page/locator-click.spec.ts` | 7 |
| `tests/page/locator-convenience.spec.ts` | 23 |
| `tests/page/locator-element-handle.spec.ts` | 5 |
| `tests/page/locator-evaluate.spec.ts` | 5 |
| `tests/page/locator-frame.spec.ts` | 26 |
| `tests/page/locator-highlight.spec.ts` | 2 |
| `tests/page/locator-is-visible.spec.ts` | 9 |
| `tests/page/locator-list.spec.ts` | 1 |
| `tests/page/locator-misc-1.spec.ts` | 15 |
| `tests/page/locator-misc-2.spec.ts` | 23 |
| `tests/page/locator-query.spec.ts` | 30 |
| `tests/page/locator-wait-for-function.spec.ts` | 10 |
| `tests/page/matchers.misc.spec.ts` | 2 |
| `tests/page/network-post-data.spec.ts` | 10 |
| `tests/page/page-add-init-script-callback.spec.ts` | 12 |
| `tests/page/page-add-init-script.spec.ts` | 12 |
| `tests/page/page-add-locator-handler.spec.ts` | 16 |
| `tests/page/page-add-script-tag.spec.ts` | 14 |
| `tests/page/page-add-style-tag.spec.ts` | 9 |
| `tests/page/page-aria-snapshot-ai.spec.ts` | 46 |
| `tests/page/page-aria-snapshot-json.spec.ts` | 11 |
| `tests/page/page-aria-snapshot.spec.ts` | 41 |
| `tests/page/page-autowaiting-basic.spec.ts` | 13 |
| `tests/page/page-autowaiting-no-hang.spec.ts` | 11 |
| `tests/page/page-basic.spec.ts` | 23 |
| `tests/page/page-cache-storage.spec.ts` | 3 |
| `tests/page/page-check.spec.ts` | 17 |
| `tests/page/page-click-during-navigation.spec.ts` | 1 |
| `tests/page/page-click-react.spec.ts` | 3 |
| `tests/page/page-click-scroll.spec.ts` | 21 |
| `tests/page/page-click-timeout-1.spec.ts` | 3 |
| `tests/page/page-click-timeout-2.spec.ts` | 2 |
| `tests/page/page-click-timeout-3.spec.ts` | 5 |
| `tests/page/page-click-timeout-4.spec.ts` | 5 |
| `tests/page/page-click.spec.ts` | 113 |
| `tests/page/page-dialog.spec.ts` | 16 |
| `tests/page/page-dispatchevent.spec.ts` | 22 |
| `tests/page/page-drag.spec.ts` | 35 |
| `tests/page/page-drop.spec.ts` | 8 |
| `tests/page/page-emulate-media.spec.ts` | 14 |
| `tests/page/page-evaluate-callback.spec.ts` | 23 |
| `tests/page/page-evaluate-handle.spec.ts` | 11 |
| `tests/page/page-evaluate-no-stall.spec.ts` | 3 |
| `tests/page/page-evaluate.spec.ts` | 110 |
| `tests/page/page-event-console.spec.ts` | 23 |
| `tests/page/page-event-load.spec.ts` | 6 |
| `tests/page/page-event-network.spec.ts` | 8 |
| `tests/page/page-event-pageerror.spec.ts` | 15 |
| `tests/page/page-event-popup.spec.ts` | 13 |
| `tests/page/page-event-request.spec.ts` | 26 |
| `tests/page/page-expose-function.spec.ts` | 21 |
| `tests/page/page-filechooser.spec.ts` | 30 |
| `tests/page/page-fill.spec.ts` | 52 |
| `tests/page/page-focus.spec.ts` | 18 |
| `tests/page/page-goto.spec.ts` | 93 |
| `tests/page/page-history.spec.ts` | 24 |
| `tests/page/page-keyboard.spec.ts` | 63 |
| `tests/page/page-leaks.spec.ts` | 9 |
| `tests/page/page-listeners.spec.ts` | 3 |
| `tests/page/page-localstorage.spec.ts` | 11 |
| `tests/page/page-mouse.spec.ts` | 25 |
| `tests/page/page-navigation.spec.ts` | 5 |
| `tests/page/page-network-idle.spec.ts` | 17 |
| `tests/page/page-network-request.spec.ts` | 47 |
| `tests/page/page-network-response.spec.ts` | 49 |
| `tests/page/page-network-sizes.spec.ts` | 17 |
| `tests/page/page-request-continue.spec.ts` | 55 |
| `tests/page/page-request-fallback.spec.ts` | 15 |
| `tests/page/page-request-fulfill.spec.ts` | 47 |
| `tests/page/page-request-gc.spec.ts` | 4 |
| `tests/page/page-request-intercept.spec.ts` | 44 |
| `tests/page/page-route.spec.ts` | 73 |
| `tests/page/page-screenshot.spec.ts` | 95 |
| `tests/page/page-select-option.spec.ts` | 34 |
| `tests/page/page-set-content.spec.ts` | 20 |
| `tests/page/page-set-extra-http-headers.spec.ts` | 6 |
| `tests/page/page-set-input-files.spec.ts` | 39 |
| `tests/page/page-strict.spec.ts` | 10 |
| `tests/page/page-wait-for-function.spec.ts` | 26 |
| `tests/page/page-wait-for-load-state.spec.ts` | 19 |
| `tests/page/page-wait-for-navigation.spec.ts` | 16 |
| `tests/page/page-wait-for-request.spec.ts` | 8 |
| `tests/page/page-wait-for-response.spec.ts` | 8 |
| `tests/page/page-wait-for-selector-1.spec.ts` | 17 |
| `tests/page/page-wait-for-selector-2.spec.ts` | 35 |
| `tests/page/page-wait-for-url.spec.ts` | 11 |
| `tests/page/queryselector.spec.ts` | 17 |
| `tests/page/retarget.spec.ts` | 15 |
| `tests/page/selectors-css.spec.ts` | 29 |
| `tests/page/selectors-frame.spec.ts` | 35 |
| `tests/page/selectors-get-by.spec.ts` | 26 |
| `tests/page/selectors-misc.spec.ts` | 23 |
| `tests/page/selectors-register.spec.ts` | 7 |
| `tests/page/selectors-role.spec.ts` | 16 |
| `tests/page/selectors-text.spec.ts` | 25 |
| `tests/page/to-match-aria-snapshot.spec.ts` | 36 |
| `tests/page/wheel.spec.ts` | 10 |
| `tests/page/workers.spec.ts` | 45 |

### Playwright `tests/library/`

The `browsercontext-*` glob expands to 33 files at the pinned commit; with
`emulation-focus`, `download` and `downloads-path` that is 36 files and 645
cases. All exist. The expected failure named in §5,
`browsercontext-locale.spec.ts` "should be isolated between contexts", is
present.

| File | Exists at pin | Cases |
|---|---|---|
| `tests/library/browsercontext-add-cookies.spec.ts` | yes | 22 |
| `tests/library/browsercontext-add-init-script.spec.ts` | yes | 12 |
| `tests/library/browsercontext-base-url.spec.ts` | yes | 8 |
| `tests/library/browsercontext-basic.spec.ts` | yes | 37 |
| `tests/library/browsercontext-clearcookies.spec.ts` | yes | 12 |
| `tests/library/browsercontext-cookies-third-party.spec.ts` | yes | 19 |
| `tests/library/browsercontext-cookies.spec.ts` | yes | 21 |
| `tests/library/browsercontext-credentials.spec.ts` | yes | 16 |
| `tests/library/browsercontext-csp.spec.ts` | yes | 4 |
| `tests/library/browsercontext-device.spec.ts` | yes | 12 |
| `tests/library/browsercontext-dsf.spec.ts` | yes | 2 |
| `tests/library/browsercontext-events.spec.ts` | yes | 22 |
| `tests/library/browsercontext-expose-function.spec.ts` | yes | 6 |
| `tests/library/browsercontext-fetch-algorithms.spec.ts` | yes | 8 |
| `tests/library/browsercontext-fetch-happy-eyeballs.spec.ts` | yes | 7 |
| `tests/library/browsercontext-fetch.spec.ts` | yes | 101 |
| `tests/library/browsercontext-har.spec.ts` | yes | 36 |
| `tests/library/browsercontext-locale.spec.ts` | yes | 18 |
| `tests/library/browsercontext-network-event.spec.ts` | yes | 9 |
| `tests/library/browsercontext-page-event.spec.ts` | yes | 11 |
| `tests/library/browsercontext-pages.spec.ts` | yes | 18 |
| `tests/library/browsercontext-proxy.spec.ts` | yes | 29 |
| `tests/library/browsercontext-reuse.spec.ts` | yes | 20 |
| `tests/library/browsercontext-route.spec.ts` | yes | 24 |
| `tests/library/browsercontext-service-worker-policy.spec.ts` | yes | 6 |
| `tests/library/browsercontext-set-extra-http-headers.spec.ts` | yes | 2 |
| `tests/library/browsercontext-storage-state.spec.ts` | yes | 28 |
| `tests/library/browsercontext-strict.spec.ts` | yes | 6 |
| `tests/library/browsercontext-timezone-id.spec.ts` | yes | 6 |
| `tests/library/browsercontext-user-agent.spec.ts` | yes | 9 |
| `tests/library/browsercontext-viewport-mobile.spec.ts` | yes | 21 |
| `tests/library/browsercontext-viewport.spec.ts` | yes | 14 |
| `tests/library/browsercontext-webauthn.spec.ts` | yes | 4 |
| `tests/library/emulation-focus.spec.ts` | yes | 12 |
| `tests/library/download.spec.ts` | yes | 54 |
| `tests/library/downloads-path.spec.ts` | yes | 9 |

### Notes for the spec owner

- No file named in §5 is missing at either pin; no rename is needed.
- Puppeteer also has `test/src/requestinterception-experimental.test.ts`,
  `touchscreen.test.ts`, `device-request-prompt.test.ts` and
  `bluetooth-emulation.test.ts` next to the named files; §5 does not list
  them, which matches the checklist (DeviceAccess and BluetoothEmulation are
  Puppeteer-only domains outside the §2 table).
