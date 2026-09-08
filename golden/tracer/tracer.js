#!/usr/bin/env node
// TentaBrowser golden-site API tracer: harness.
//
// Opens a site scenario in headless Chromium (Playwright), injects
// inject.js into every window realm before any page script and, best
// effort, into workers, drives the scenario from sites.js, collects the
// per-realm dumps plus network characteristics observed through CDP, and
// writes one JSON inventory sorted by count.
//
//   node tracer.js <site> [--out <file>] [--scratch <dir>] [--headed]
//   node tracer.js url:<https://...> --out /tmp/x.json
'use strict';

const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');
const SITES = require('./sites');
const { scrubMeta } = require('./scrub');

const TRACER_VERSION = '0.1.0';
const INJECT_SOURCE = fs.readFileSync(path.join(__dirname, 'inject.js'), 'utf8');
const FLUSH_PREFIX = '__TENTA_TRACE_FLUSH__ ';
const DUMP_FN = () => {
  const t = globalThis[Symbol.for('tenta.tracer')];
  return t ? t.dump(true) : null;
};

function parseArgs(argv) {
  const args = { site: null, out: null, scratch: null, headed: false, verbose: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--out') args.out = argv[++i];
    else if (a === '--scratch') args.scratch = argv[++i];
    else if (a === '--headed') args.headed = true;
    else if (a === '--verbose') args.verbose = true;
    else if (!args.site) args.site = a;
    else throw new Error(`unexpected argument ${a}`);
  }
  if (!args.site) throw new Error('usage: node tracer.js <site|url:...> [--out file] [--scratch dir] [--headed]');
  return args;
}

// ---- accumulation ----------------------------------------------------------

// Keys are site-controlled strings ("constructor" included), so every map is
// consulted with hasOwn and created without a prototype.
const own = (o, k) => (Object.hasOwn(o, k) ? o[k] : undefined);
const nullObj = () => Object.create(null);

function addCounts(dst, src) {
  for (const [k, v] of Object.entries(src || {})) {
    if (typeof v === 'number') dst[k] = (own(dst, k) || 0) + v;
    else if (Array.isArray(v)) dst[k] = (own(dst, k) || []).concat(v);
    else if (v && typeof v === 'object') addCounts(own(dst, k) || (dst[k] = nullObj()), v);
  }
  return dst;
}

const SECTIONS = ['events', 'elements', 'custom_elements', 'css', 'media', 'audio', 'canvas', 'webgl',
  'workers', 'wasm', 'intl', 'rtc', 'storage', 'navigator', 'permissions', 'websockets', 'event_sources'];

class Inventory {
  constructor(site, spec) {
    this.meta = {
      site,
      title: spec.title,
      tracer_version: TRACER_VERSION,
      playwright_version: require('playwright/package.json').version,
      chromium_version: null,
      channel: 'chromium',
      user_agent: null,
      user_agent_raw: null,
      date: new Date().toISOString(),
      duration_ms: 0,
      viewport: { width: 1280, height: 800 },
      locale: 'en-US',
      urls: spec.urls || [],
      navigations: [],
      final_url: null,
      page_title: null,
      steps: [],
      notes: [],
      limitations: [
        'Dedicated, shared and service workers are traced after they attach (Playwright resumes them before the tracer can run), so their first synchronous statements are missed; workers that terminate before a harvest lose the counts since the previous harvest.',
        'AudioWorklet and PaintWorklet global scopes are not traced (no injection path); worklet module URLs and AudioWorkletNode names are recorded from the page side.',
        '[LegacyUnforgeable] members (Location.*, Window.location/document/window/top, Document.location) are non-configurable and cannot be wrapped; see meta.unwrappable.',
        'ECMAScript intrinsics are not traced except Intl, the locale-sensitive Date/Number/String/Array methods, WebAssembly, SharedArrayBuffer and Atomics.wait/notify/waitAsync.',
        'Cross-origin window property access (e.g. otherWindow.postMessage) goes through cross-origin wrappers, not the wrapped prototypes, and is undercounted.',
        'HTTP protocol versions come from CDP Network.responseReceived on the main frame session plus best-effort sessions on out-of-process iframes; requests of frames that could not be attached are counted without a protocol.',
        'Stylesheets are walked once per CSSStyleSheet object at harvest time; cross-origin sheets without CORS are counted in css.stylesheets.cross_origin_skipped only.',
        'The User-Agent string had "HeadlessChrome" replaced by "Chrome" (Playwright then derives consistent navigator.userAgentData); no other fingerprint changes were made.',
      ].concat(spec.limitations || []),
      page_errors: { count: 0, samples: [] },
      console_errors: 0,
      frames_traced: 0,
      workers_traced: 0,
      service_workers_traced: 0,
      unwrappable: [],
      tracer_errors: [],
      wrapped_members_top_frame: 0,
      tracer_install_ms_top_frame: null,
    };
    this.interfaces = nullObj();
    for (const s of SECTIONS) this[s] = nullObj();
    this.network = {
      requests: 0, responses: 0, failed: 0, bytes_encoded: 0,
      http_versions: {}, resource_types: {}, methods: {}, status: {}, hosts: {}, host_protocols: {},
      mime_types: {}, content_encodings: {}, alt_svc_hosts: {}, from_service_worker: 0, from_cache: 0,
      websockets: {}, ws_frames_sent: 0, ws_frames_received: 0, failed_errors: {}, failed_hosts: {},
    };
    this.realms = nullObj();
    this._topUnwrappable = null;
  }

  mergeDump(d, source) {
    if (!d || !d.realm) return;
    const realm = d.realm;
    const isTop = realm.kind === 'window' && realm.is_top;
    let realmTotal = 0;
    for (const [iface, members] of Object.entries(d.interfaces || {})) {
      const entry = own(this.interfaces, iface) || (this.interfaces[iface] = { total: 0, total_top: 0, realms: nullObj(), members: nullObj() });
      for (const [member, counts] of Object.entries(members)) {
        const m = own(entry.members, member) || (entry.members[member] = nullObj());
        let n = 0;
        for (const kind of ['get', 'set', 'call', 'construct']) {
          if (counts[kind]) { m[kind] = (m[kind] || 0) + counts[kind]; n += counts[kind]; }
        }
        if (counts.static) m.static = true;
        entry.total += n;
        if (isTop) entry.total_top += n;
        entry.realms[realm.kind] = (own(entry.realms, realm.kind) || 0) + n;
        realmTotal += n;
      }
    }
    for (const s of SECTIONS) addCounts(this[s], d[s]);
    const rk = `${realm.kind} ${realm.origin || realm.href || '?'} ${realm.name} ${isTop ? 'top' : 'sub'}`;
    const r = own(this.realms, rk) || (this.realms[rk] = { kind: realm.kind, global: realm.name, origin: realm.origin, is_top: !!realm.is_top, dumps: 0, member_accesses: 0, wrapped_count: d.wrapped_count, install_ms: d.install_ms, sources: {} });
    r.dumps++;
    r.member_accesses += realmTotal;
    r.sources[source] = (r.sources[source] || 0) + 1;
    if (isTop && !this._topUnwrappable) {
      this._topUnwrappable = d.unwrappable || [];
      this.meta.wrapped_members_top_frame = d.wrapped_count;
      this.meta.tracer_install_ms_top_frame = d.install_ms;
    }
    for (const e of d.errors || []) if (this.meta.tracer_errors.length < 50 && !this.meta.tracer_errors.includes(e)) this.meta.tracer_errors.push(e);
  }

  finalize() {
    const sortNum = (o) => Object.fromEntries(Object.entries(o).sort((a, b) => (b[1] - a[1]) || (a[0] < b[0] ? -1 : 1)));
    const sortDeep = (o) => {
      if (Array.isArray(o) || !o || typeof o !== 'object') return o;
      const vals = Object.values(o);
      if (vals.length && vals.every((v) => typeof v === 'number')) return sortNum(o);
      const out = nullObj();
      for (const k of Object.keys(o)) out[k] = sortDeep(o[k]);
      return out;
    };
    const interfaces = nullObj();
    for (const [iface, e] of Object.entries(this.interfaces).sort((a, b) => (b[1].total - a[1].total) || (a[0] < b[0] ? -1 : 1))) {
      const members = nullObj();
      const sum = (m) => (m.get || 0) + (m.set || 0) + (m.call || 0) + (m.construct || 0);
      for (const [name, m] of Object.entries(e.members).sort((a, b) => (sum(b[1]) - sum(a[1])) || (a[0] < b[0] ? -1 : 1))) members[name] = m;
      interfaces[iface] = { total: e.total, total_top: e.total_top, member_count: Object.keys(members).length, realms: sortNum(e.realms), members };
    }
    this.meta.unwrappable = this._topUnwrappable || [];
    this.meta.navigations = this.meta.navigations.filter((n, i, a) => i === 0 || n.url !== a[i - 1].url);
    this.meta.interface_count = Object.keys(interfaces).length;
    const out = { meta: this.meta, interfaces };
    for (const s of SECTIONS) out[s] = sortDeep(this[s]);
    out.network = sortDeep(this.network);
    out.realms = Object.values(this.realms).sort((a, b) => b.member_accesses - a.member_accesses);
    return out;
  }
}

// ---- network observation ---------------------------------------------------

function hostOf(url) { try { return new URL(url).host; } catch (_) { return '?'; } }

async function attachNetwork(session, inv, label) {
  const pending = new Map();
  session.on('Network.responseReceived', (ev) => {
    const r = ev.response;
    const n = inv.network;
    n.responses++;
    const proto = r.protocol || 'unknown';
    n.http_versions[proto] = (n.http_versions[proto] || 0) + 1;
    const host = hostOf(r.url);
    const hp = n.host_protocols[host] || (n.host_protocols[host] = {});
    hp[proto] = (hp[proto] || 0) + 1;
    const mime = (r.mimeType || '?').split(';')[0];
    n.mime_types[mime] = (n.mime_types[mime] || 0) + 1;
    const headers = Object.fromEntries(Object.entries(r.headers || {}).map(([k, v]) => [k.toLowerCase(), v]));
    if (headers['content-encoding']) n.content_encodings[headers['content-encoding']] = (n.content_encodings[headers['content-encoding']] || 0) + 1;
    if (headers['alt-svc']) n.alt_svc_hosts[host] = (n.alt_svc_hosts[host] || 0) + 1;
    if (r.fromServiceWorker) n.from_service_worker++;
    if (r.fromDiskCache || r.fromPrefetchCache) n.from_cache++;
    pending.set(ev.requestId, true);
  });
  session.on('Network.loadingFinished', (ev) => {
    if (pending.delete(ev.requestId)) inv.network.bytes_encoded += ev.encodedDataLength || 0;
  });
  session.on('Network.webSocketCreated', (ev) => {
    const w = inv.network.websockets[ev.url] || (inv.network.websockets[ev.url] = { frames_sent: 0, frames_received: 0, session: label });
    w.created = (w.created || 0) + 1;
  });
  session.on('Network.webSocketFrameSent', (ev) => { inv.network.ws_frames_sent++; });
  session.on('Network.webSocketFrameReceived', (ev) => { inv.network.ws_frames_received++; });
  await session.send('Network.enable');
}

// ---- harness ---------------------------------------------------------------

async function withTimeout(promise, ms, fallback) {
  let timer;
  const t = new Promise((resolve) => { timer = setTimeout(() => resolve(fallback), ms); });
  try { return await Promise.race([promise, t]); } finally { clearTimeout(timer); }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const spec = args.site.startsWith('url:')
    ? { title: args.site.slice(4), urls: [args.site.slice(4)], run: SITES.genericRun(args.site.slice(4)) }
    : SITES[args.site];
  if (!spec) throw new Error(`unknown site ${args.site}; known: ${Object.keys(SITES).filter((k) => k !== 'genericRun').join(', ')}`);
  const scratch = args.scratch || path.join(__dirname, 'out');
  fs.mkdirSync(scratch, { recursive: true });
  const outFile = args.out || path.join(__dirname, '..', 'inventory', `${args.site}.json`);
  const t0 = Date.now();
  const inv = new Inventory(args.site, spec);
  const log = (...a) => console.error(`[tracer ${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...a);

  const browser = await chromium.launch({
    channel: 'chromium',
    headless: !args.headed,
    args: ['--autoplay-policy=no-user-gesture-required'],
  });
  inv.meta.chromium_version = browser.version();
  const probe = await browser.newPage();
  inv.meta.user_agent_raw = await probe.evaluate(() => navigator.userAgent);
  await probe.context().close();
  inv.meta.user_agent = inv.meta.user_agent_raw.replace('HeadlessChrome', 'Chrome');

  const context = await browser.newContext({
    viewport: inv.meta.viewport,
    locale: inv.meta.locale,
    userAgent: inv.meta.user_agent,
    serviceWorkers: 'allow',
  });
  await context.addInitScript({ content: INJECT_SOURCE });

  const workers = new Set();
  const serviceWorkers = new Set();
  context.on('serviceworker', async (sw) => {
    try { await sw.evaluate(INJECT_SOURCE); serviceWorkers.add(sw); log('service worker traced', sw.url()); }
    catch (e) { inv.meta.notes.push(`service worker injection failed (${sw.url()}): ${e.message.split('\n')[0]}`); }
  });

  const page = await context.newPage();
  page.on('worker', async (w) => {
    try { await w.evaluate(INJECT_SOURCE); workers.add(w); log('worker traced', w.url()); }
    catch (e) { inv.meta.notes.push(`worker injection failed (${w.url()}): ${e.message.split('\n')[0]}`); }
  });
  page.on('console', (msg) => {
    if (msg.type() === 'error') inv.meta.console_errors++;
    const text = msg.text();
    if (!text.startsWith(FLUSH_PREFIX)) { if (args.verbose) log(`console.${msg.type()}: ${text.slice(0, 300)}`); return; }
    try {
      const d = JSON.parse(text.slice(FLUSH_PREFIX.length));
      inv.mergeDump(d, 'unload_flush');
      inv.meta.frames_traced++;
      // The document's workers die with it; grab what they have while they are still alive.
      if (d.realm && d.realm.is_top && workers.size) harvestWorkers('unload_flush').catch(() => {});
    } catch (e) { inv.meta.notes.push(`unload flush parse failed: ${e.message}`); }
  });
  page.on('pageerror', (err) => {
    if (args.verbose) log('pageerror:', String(err.message || err).slice(0, 300));
    inv.meta.page_errors.count++;
    if (inv.meta.page_errors.samples.length < 20) inv.meta.page_errors.samples.push(String(err.message || err).slice(0, 300));
  });
  page.on('crash', () => inv.meta.notes.push('page crashed'));
  page.on('framenavigated', (f) => { if (f === page.mainFrame()) inv.meta.navigations.push({ url: f.url(), at_ms: Date.now() - t0 }); });
  page.on('request', (r) => {
    const n = inv.network;
    n.requests++;
    n.resource_types[r.resourceType()] = (n.resource_types[r.resourceType()] || 0) + 1;
    n.methods[r.method()] = (n.methods[r.method()] || 0) + 1;
    const host = hostOf(r.url());
    n.hosts[host] = (n.hosts[host] || 0) + 1;
  });
  page.on('response', (r) => { inv.network.status[r.status()] = (inv.network.status[r.status()] || 0) + 1; });
  page.on('requestfailed', (r) => {
    inv.network.failed++;
    const e = r.failure() ? r.failure().errorText : 'unknown';
    inv.network.failed_errors[e] = (own(inv.network.failed_errors, e) || 0) + 1;
    const host = hostOf(r.url());
    inv.network.failed_hosts[host] = (own(inv.network.failed_hosts, host) || 0) + 1;
  });
  page.on('websocket', (ws) => {
    const w = inv.network.websockets[ws.url()] || (inv.network.websockets[ws.url()] = { frames_sent: 0, frames_received: 0 });
    w.seen_by_playwright = (w.seen_by_playwright || 0) + 1;
    ws.on('framesent', () => { w.frames_sent++; });
    ws.on('framereceived', () => { w.frames_received++; });
  });

  const mainSession = await context.newCDPSession(page);
  await attachNetwork(mainSession, inv, 'main');
  page.on('frameattached', async (f) => {
    try { const s = await context.newCDPSession(f); await attachNetwork(s, inv, 'oopif'); }
    catch (_) { /* same-process frame: covered by the main session */ }
  });

  const harvestWorkers = async (label) => {
    let ws = 0, sws = 0;
    await Promise.all([...workers].map(async (w) => {
      const d = await withTimeout(w.evaluate(DUMP_FN).catch(() => null), 15000, null);
      if (d) { inv.mergeDump(d, label); ws++; } else workers.delete(w);
    }));
    await Promise.all([...serviceWorkers].map(async (sw) => {
      const d = await withTimeout(sw.evaluate(DUMP_FN).catch(() => null), 15000, null);
      if (d) { inv.mergeDump(d, label); sws++; }
    }));
    return { ws, sws };
  };
  const harvest = async (label) => {
    const at = Date.now() - t0;
    let frames = 0;
    for (const f of page.frames()) {
      const d = await withTimeout(f.evaluate(DUMP_FN).catch(() => null), 15000, null);
      if (d) { inv.mergeDump(d, label); frames++; }
    }
    const { ws, sws } = await harvestWorkers(label);
    inv.meta.frames_traced += frames;
    inv.meta.workers_traced = Math.max(inv.meta.workers_traced, ws);
    inv.meta.service_workers_traced = Math.max(inv.meta.service_workers_traced, sws);
    inv.meta.steps.push({ label, at_ms: at, url: page.url(), frames, workers: ws, service_workers: sws });
    log(`harvest ${label}: ${frames} frames, ${ws} workers, ${sws} service workers`);
  };

  const h = {
    page, context, inv, log,
    note: (text) => { inv.meta.notes.push(text); log('note:', text); },
    limitation: (text) => { inv.meta.limitations.push(text); log('limitation:', text); },
    harvest,
    sleep: (ms) => page.waitForTimeout(ms),
    goto: async (url, opts = {}) => {
      try {
        const resp = await page.goto(url, { waitUntil: opts.waitUntil || 'load', timeout: opts.timeout || 60000 });
        h.note(`goto ${url} -> ${resp ? resp.status() : 'no response'} ${page.url()}`);
        return resp;
      } catch (e) {
        h.note(`goto ${url} failed: ${e.message.split('\n')[0]}`);
        return null;
      }
    },
    settle: async (ms) => {
      await page.waitForLoadState('networkidle', { timeout: ms }).catch(() => {});
      await page.waitForTimeout(Math.min(ms, 3000));
    },
    consent: async (selectors) => {
      for (const sel of selectors) {
        const loc = page.locator(sel).first();
        const visible = await loc.waitFor({ state: 'visible', timeout: 1500 }).then(() => true).catch(() => false);
        if (!visible) continue;
        try {
          await loc.click({ timeout: 5000 });
          h.note(`consent: clicked ${sel}`);
          await page.waitForLoadState('load', { timeout: 30000 }).catch(() => {});
          return true;
        } catch (e) { h.note(`consent: click ${sel} failed: ${e.message.split('\n')[0]}`); }
      }
      return false;
    },
    scroll: async (steps = 4, dy = 600) => {
      for (let i = 0; i < steps; i++) { await page.mouse.wheel(0, dy); await page.waitForTimeout(500); }
    },
    screenshot: async (name) => {
      try { await page.screenshot({ path: path.join(scratch, `${args.site}-${name}.png`) }); } catch (_) {}
    },
  };

  try {
    await spec.run(h);
  } catch (e) {
    h.note(`scenario error: ${e.message.split('\n')[0]}`);
  }
  await harvest('final');
  try {
    inv.meta.final_url = page.url();
    inv.meta.page_title = await withTimeout(page.title().catch(() => null), 5000, null);
    inv.navigator.user_agent_data = await withTimeout(page.evaluate(async () => {
      const t = globalThis[Symbol.for('tenta.tracer')];
      const was = t ? t.enabled : true;
      if (t) t.enabled = false;
      try {
        const u = navigator.userAgentData;
        if (!u) return null;
        const hints = ['architecture', 'bitness', 'model', 'platformVersion', 'uaFullVersion', 'fullVersionList', 'wow64', 'formFactors'];
        return { low_entropy: u.toJSON(), high_entropy: await u.getHighEntropyValues(hints).catch((e) => String(e)) };
      } finally { if (t) t.enabled = was; }
    }).catch(() => null), 10000, null);
  } catch (_) {}
  await h.screenshot('final');
  inv.meta.duration_ms = Date.now() - t0;
  await context.close().catch(() => {});
  await browser.close().catch(() => {});

  const out = inv.finalize();
  scrubMeta(out.meta);
  fs.mkdirSync(path.dirname(outFile), { recursive: true });
  fs.writeFileSync(outFile, JSON.stringify(out, null, 1) + '\n');
  const top = Object.entries(out.interfaces).slice(0, 20).map(([k, v]) => `${k}=${v.total}`).join(' ');
  log(`wrote ${outFile}: ${out.meta.interface_count} interfaces; top: ${top}`);
}

main().catch((e) => { console.error(e); process.exit(1); });
