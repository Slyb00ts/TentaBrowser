// Load a Google SERP in headless Chromium through Playwright, dismiss the
// consent interstitial if one appears, and print a JSON record on stdout:
// status, final URL, response headers (cookie values removed), whether a
// consent / CAPTCHA / JS-required page was seen, timing and the full HTML.
//
// Usage (called by providers/survey.py):
//   node google_chromium.mjs --url <serp-url> --accept-language pl-PL,pl;q=0.9 \
//        --locale pl-PL --user-agent "<ua>"
//
// The Chromium binary comes from $TENTA_CHROMIUM, otherwise the newest
// Playwright build under ~/Library/Caches/ms-playwright (macOS) or
// ~/.cache/ms-playwright (Linux) is used, so no download happens here.

import { chromium } from "playwright-core";
import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

function arg(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function findChromium() {
  if (process.env.TENTA_CHROMIUM) return process.env.TENTA_CHROMIUM;
  const roots = [
    join(homedir(), "Library", "Caches", "ms-playwright"),
    join(homedir(), ".cache", "ms-playwright"),
  ];
  const candidates = [];
  for (const root of roots) {
    if (!existsSync(root)) continue;
    for (const dir of readdirSync(root)) {
      const match = /^chromium-(\d+)$/.exec(dir);
      if (!match) continue;
      const mac = join(root, dir, "chrome-mac-arm64", "Google Chrome for Testing.app", "Contents", "MacOS", "Google Chrome for Testing");
      const macIntel = join(root, dir, "chrome-mac", "Chromium.app", "Contents", "MacOS", "Chromium");
      const linux = join(root, dir, "chrome-linux", "chrome");
      for (const path of [mac, macIntel, linux]) {
        if (existsSync(path)) candidates.push({ revision: Number(match[1]), path });
      }
    }
  }
  candidates.sort((a, b) => b.revision - a.revision);
  if (!candidates.length) throw new Error("no Playwright Chromium build found; set TENTA_CHROMIUM");
  return candidates[0].path;
}

const CONSENT_BUTTONS = [
  "button#W0wltc",                       // "Reject all"
  "button:has-text('Reject all')",
  "button:has-text('Odrzuć wszystko')",
  "form[action*='consent'] button",
];

const url = arg("--url");
const acceptLanguage = arg("--accept-language", "en-US,en;q=0.9");
const locale = arg("--locale", "en-US");
const userAgent = arg("--user-agent");
const executablePath = findChromium();

const record = { url, executable: executablePath, redirects: [], consent_seen: false, consent_dismissed: false, notes: [] };
const started = performance.now();
const browser = await chromium.launch({ headless: true, executablePath });
record.browser_version = browser.version();
const context = await browser.newContext({
  userAgent,
  locale,
  extraHTTPHeaders: { "Accept-Language": acceptLanguage },
  viewport: { width: 1280, height: 900 },
});
const page = await context.newPage();
page.on("response", (response) => {
  const status = response.status();
  if (status >= 300 && status < 400 && response.request().isNavigationRequest()) {
    record.redirects.push({ status, from: response.url(), to: response.headers()["location"] || "" });
  }
});

try {
  const response = await page.goto(url, { waitUntil: "domcontentloaded", timeout: 45000 });
  record.ttfb_ms = Math.round(performance.now() - started);
  record.status = response ? response.status() : null;
  record.final_url = page.url();
  if (/consent\.google\./.test(page.url()) || (await page.locator("form[action*='consent']").count()) > 0) {
    record.consent_seen = true;
    for (const selector of CONSENT_BUTTONS) {
      const button = page.locator(selector).first();
      if ((await button.count()) > 0) {
        await Promise.all([
          page.waitForNavigation({ waitUntil: "domcontentloaded", timeout: 30000 }).catch(() => null),
          button.click(),
        ]);
        record.consent_dismissed = true;
        record.notes.push(`consent dismissed via ${selector}`);
        break;
      }
    }
  }
  await page.waitForSelector("#rso, #search, form#captcha-form, #sorry, div#main", { timeout: 15000 }).catch(() => {
    record.notes.push("no result container within 15 s");
  });
  await page.waitForLoadState("networkidle", { timeout: 10000 }).catch(() => record.notes.push("network not idle after 10 s"));
  record.final_url = page.url();
  record.title = await page.title();
  record.captcha = /\/sorry\//.test(page.url()) || (await page.locator("form#captcha-form, #recaptcha").count()) > 0;
  record.js_required = (await page.locator("#enablejs, a[href*='enablejs']").count()) > 0;
  record.h3_count = await page.locator("#rso h3").count();
  record.cookies_set = (await context.cookies()).map((cookie) => cookie.name).sort();
  const headers = response ? await response.allHeaders() : {};
  const scrubbed = {};
  const cookieNames = [];
  for (const [key, value] of Object.entries(headers)) {
    if (key === "set-cookie") cookieNames.push(...value.split("\n").map((line) => line.split("=")[0].trim()));
    else scrubbed[key] = value;
  }
  if (cookieNames.length) scrubbed["set-cookie"] = "<" + [...new Set(cookieNames)].join(", ") + ">";
  record.response_headers = scrubbed;
  record.body = await page.content();
  record.body_bytes = Buffer.byteLength(record.body, "utf8");
} catch (error) {
  record.error = String(error && error.message ? error.message : error);
  record.body = record.body || (await page.content().catch(() => ""));
} finally {
  record.total_ms = Math.round(performance.now() - started);
  await browser.close();
}
process.stdout.write(JSON.stringify(record));
