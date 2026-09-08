#!/usr/bin/env node
// Capture Chromium reference renders for the render fixture corpus.
//
// For every `render-fixtures/<name>/index.html` this writes
//   <name>/chromium.png   viewport screenshot (900x1000, DPR 1)
//   <name>/checks.json    structural checks: the bounding client rect of every
//                         element under <body> (`named: true` when it carries an
//                         `id` or `data-check` attribute, in which case the
//                         selector is that id/attribute) and the first client
//                         rect of every non-blank text node
// and `render-fixtures/captures.json` recording the browser and settings.
//
// The settings match the reference corpus capture recipe
// (obscura-reference/render-repros/capture_chromium.py): 900x1000 viewport,
// device scale factor 1, en-US, UTC, light colour scheme, `load` plus a
// 2000 ms settle. Remote network is blocked so captures are offline and
// deterministic; `data:` URLs and sibling files still load.
//
// Usage: node capture.mjs [--fixture NAME ...] [--settle-ms N]
//        [--channel chrome|chromium] [--executable PATH]
// See docs/dev/render-fixtures.md.

import { chromium } from "playwright";
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const VIEWPORT = { width: 900, height: 1000 };
const DEFAULT_SETTLE_MS = 2000;
const NAVIGATION_TIMEOUT_MS = 30_000;

const toolsDir = dirname(fileURLToPath(import.meta.url));
const corpusDir = resolve(toolsDir, "..");

function parseArgs(argv) {
  const options = { fixtures: [], settleMs: DEFAULT_SETTLE_MS, channel: "chrome", executable: null };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length) throw new Error(`missing value for ${arg}`);
      return argv[i];
    };
    if (arg === "--fixture") options.fixtures.push(next());
    else if (arg === "--settle-ms") options.settleMs = Number(next());
    else if (arg === "--channel") options.channel = next();
    else if (arg === "--executable") options.executable = next();
    else throw new Error(`unknown argument ${arg}`);
  }
  return options;
}

function listFixtures() {
  return readdirSync(corpusDir)
    .filter((name) => {
      const dir = join(corpusDir, name);
      return statSync(dir).isDirectory() && name !== "tools" && statSync(join(dir, "index.html"), { throwIfNoEntry: false });
    })
    .sort();
}

// Runs inside the page. Returns rects in CSS pixels relative to the viewport
// (the page is not scrolled), rounded to 1/100 px.
function extractChecks() {
  const round = (v) => Math.round(v * 100) / 100;
  const rect = (r) => ({ x: round(r.x), y: round(r.y), width: round(r.width), height: round(r.height) });
  const cssPath = (el) => {
    const parts = [];
    for (let node = el; node && node.nodeType === Node.ELEMENT_NODE; node = node.parentElement) {
      if (node.id) {
        parts.unshift(`#${CSS.escape(node.id)}`);
        break;
      }
      const tag = node.localName;
      const parent = node.parentElement;
      if (!parent) {
        parts.unshift(tag);
        break;
      }
      const index = Array.prototype.indexOf.call(parent.children, node) + 1;
      parts.unshift(`${tag}:nth-child(${index})`);
    }
    return parts.join(" > ");
  };
  const checks = [];
  const root = document.body ?? document.documentElement;
  for (const el of root.querySelectorAll("*")) {
    if (el.closest("script, style, template, noscript")) continue;
    const named = el.hasAttribute("id") || el.hasAttribute("data-check");
    const selector = el.id
      ? `#${CSS.escape(el.id)}`
      : el.hasAttribute("data-check")
        ? `[data-check="${el.getAttribute("data-check")}"]`
        : cssPath(el);
    checks.push({ selector, kind: "element", tag: el.localName, named, ...rect(el.getBoundingClientRect()) });
  }
  const skip = new Set(["script", "style", "template", "noscript", "title", "head"]);
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const perParent = new Map();
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node.data.replace(/\s+/g, " ").trim();
    if (!text) continue;
    const parent = node.parentElement;
    if (!parent || skip.has(parent.localName) || parent.closest("script, style, template, noscript")) continue;
    const index = perParent.get(parent) ?? 0;
    perParent.set(parent, index + 1);
    const range = document.createRange();
    range.selectNodeContents(node);
    const first = range.getClientRects()[0];
    if (!first || (first.width === 0 && first.height === 0)) continue;
    checks.push({
      selector: `${cssPath(parent)}::text(${index})`,
      kind: "text",
      text: text.length > 80 ? `${text.slice(0, 77)}...` : text,
      ...rect(first),
    });
  }
  return {
    scroll_size: { width: round(document.documentElement.scrollWidth), height: round(document.documentElement.scrollHeight) },
    checks,
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const fixtures = options.fixtures.length ? options.fixtures : listFixtures();
  const launch = { headless: true };
  if (options.executable) launch.executablePath = options.executable;
  else launch.channel = options.channel;
  const browser = await chromium.launch(launch);
  const context = await browser.newContext({
    viewport: VIEWPORT,
    deviceScaleFactor: 1,
    locale: "en-US",
    timezoneId: "UTC",
    colorScheme: "light",
    reducedMotion: "no-preference",
  });
  await context.route(/^https?:/, (route) => route.abort("blockedbyclient"));
  const page = await context.newPage();
  const consoleErrors = [];
  page.on("pageerror", (error) => consoleErrors.push(String(error)));

  const manifest = {
    browser: { name: options.executable ? "custom" : options.channel, version: browser.version(), playwright: playwrightVersion() },
    settings: {
      viewport: [VIEWPORT.width, VIEWPORT.height],
      device_scale_factor: 1,
      locale: "en-US",
      timezone_id: "UTC",
      color_scheme: "light",
      reduced_motion: "no-preference",
      wait_until: "load",
      settle_ms: options.settleMs,
      remote_network: "blocked",
      screenshot: "viewport",
    },
    captured_at: new Date().toISOString(),
    fixtures: {},
  };

  for (const name of fixtures) {
    const dir = join(corpusDir, name);
    const url = pathToFileURL(join(dir, "index.html")).href;
    consoleErrors.length = 0;
    await page.goto(url, { waitUntil: "load", timeout: NAVIGATION_TIMEOUT_MS });
    await page.waitForTimeout(options.settleMs);
    const extracted = await page.evaluate(extractChecks);
    await page.screenshot({ path: join(dir, "chromium.png"), fullPage: false, animations: "allow", caret: "hide" });
    const checks = {
      fixture: name,
      viewport: [VIEWPORT.width, VIEWPORT.height],
      device_scale_factor: 1,
      scroll_size: extracted.scroll_size,
      checks: extracted.checks,
    };
    writeFileSync(join(dir, "checks.json"), `${JSON.stringify(checks, null, 2)}\n`);
    manifest.fixtures[name] = { checks: extracted.checks.length, page_errors: consoleErrors.slice() };
    console.log(`${name}: ${extracted.checks.length} checks${consoleErrors.length ? ` (${consoleErrors.length} page errors)` : ""}`);
  }

  writeFileSync(join(corpusDir, "captures.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  await context.close();
  await browser.close();
}

function playwrightVersion() {
  return JSON.parse(readFileSync(join(toolsDir, "node_modules", "playwright", "package.json"), "utf8")).version;
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
