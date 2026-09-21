#!/usr/bin/env node
// WPT browser driver for `--product chromium` (see wpt/run.py and docs/dev/wpt.md).
//
// Protocol: `node driver-chromium.mjs <plan.json> <results.json>`.
//   plan.json    { "base": "http://web-platform.test:8000",
//                  "resolver_rules": true | false,
//                  "jobs": 4,
//                  "deadline_epoch_ms": 1700000000000 | null,  (no new tests start after it)
//                  "channel": "chromium" | "chrome" | ...,   (optional)
//                  "executable": "/path/to/chrome",           (optional)
//                  "tests": [ { "url": "/dom/nodes/x.html", "timeout_ms": 10000 }, ... ] }
//   results.json { "browser": { "name", "version" },
//                  "tests": [ { "test", "status", "message", "duration_ms",
//                               "subtests": [ { "name", "status", "message" } ] } ] }
//
// Every test runs in a fresh page. The standard `resources/testharnessreport.js`
// is replaced on the wire by a completion hook that hands the testharness
// results to the driver through an exposed binding. The `tentabrowser` product
// implements the same contract natively, with no binding: its own request
// interceptor answers that path, and the hook posts the result back with
// `fetch()` (docs/dev/wpt.md).

import { chromium } from "playwright";
import { readFileSync, writeFileSync } from "node:fs";

const HARNESS_STATUS = ["OK", "ERROR", "TIMEOUT", "PRECONDITION_FAILED"];
const SUBTEST_STATUS = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];
const BINDING = "__tentaWptResult";

// Served in place of resources/testharnessreport.js.
const REPORT_HOOK = `
setup({ output: false });
add_completion_callback(function (tests, harnessStatus) {
  var payload = {
    status: harnessStatus.status,
    message: harnessStatus.message,
    tests: tests.map(function (t) { return { name: t.name, status: t.status, message: t.message }; })
  };
  if (window.${BINDING}) { window.${BINDING}(JSON.stringify(payload)); }
});
`;

async function runOne(context, base, test) {
  const page = await context.newPage();
  const started = Date.now();
  let settle;
  const done = new Promise((resolve) => {
    settle = resolve;
  });
  await page.exposeFunction(BINDING, (json) => settle({ kind: "result", payload: JSON.parse(json) }));
  page.on("crash", () => settle({ kind: "crash" }));
  const timer = setTimeout(() => settle({ kind: "timeout" }), test.timeout_ms);
  let outcome;
  try {
    await page.goto(base + test.url, { waitUntil: "commit", timeout: test.timeout_ms });
    outcome = await done;
  } catch (error) {
    outcome = { kind: "error", message: String(error) };
  } finally {
    clearTimeout(timer);
    await page.close().catch(() => {});
  }
  const duration_ms = Date.now() - started;
  if (outcome.kind === "result") {
    return {
      test: test.url,
      status: HARNESS_STATUS[outcome.payload.status] ?? "ERROR",
      message: outcome.payload.message ?? null,
      duration_ms,
      subtests: outcome.payload.tests.map((t) => ({
        name: t.name,
        status: SUBTEST_STATUS[t.status] ?? "FAIL",
        message: t.message ?? null,
      })),
    };
  }
  const status = outcome.kind === "crash" ? "CRASH" : outcome.kind === "timeout" ? "TIMEOUT" : "ERROR";
  return { test: test.url, status, message: outcome.message ?? null, duration_ms, subtests: [] };
}

async function main() {
  const [planPath, resultsPath] = process.argv.slice(2);
  if (!planPath || !resultsPath) throw new Error("usage: driver-chromium.mjs <plan.json> <results.json>");
  const plan = JSON.parse(readFileSync(planPath, "utf8"));
  const args = [];
  if (plan.resolver_rules) {
    args.push(
      "--host-resolver-rules=MAP web-platform.test 127.0.0.1, MAP *.web-platform.test 127.0.0.1, MAP not-web-platform.test 127.0.0.1, MAP *.not-web-platform.test 127.0.0.1",
    );
  }
  const launch = { headless: true, args };
  if (plan.executable) launch.executablePath = plan.executable;
  else if (plan.channel) launch.channel = plan.channel;
  const browser = await chromium.launch(launch);
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  await context.route(/\/resources\/testharnessreport\.js(\?.*)?$/, (route) =>
    route.fulfill({ status: 200, contentType: "text/javascript", body: REPORT_HOOK }),
  );

  const queue = plan.tests.slice();
  const results = [];
  let completed = 0;
  const deadline = plan.deadline_epoch_ms ?? Infinity;
  const worker = async () => {
    for (let test = queue.shift(); test; test = queue.shift()) {
      const result =
        Date.now() < deadline
          ? await runOne(context, plan.base, test)
          : { test: test.url, status: "NOTRUN", message: "run deadline reached", duration_ms: 0, subtests: [] };
      results.push(result);
      completed += 1;
      const passed = result.subtests.filter((s) => s.status === "PASS").length;
      process.stderr.write(`[${completed}/${plan.tests.length}] ${result.status.padEnd(7)} ${passed}/${result.subtests.length} ${result.test}\n`);
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, plan.jobs ?? 4) }, worker));
  results.sort((a, b) => (a.test < b.test ? -1 : a.test > b.test ? 1 : 0));

  writeFileSync(
    resultsPath,
    `${JSON.stringify({ browser: { name: plan.executable ? "custom" : plan.channel ?? "chromium", version: browser.version() }, tests: results }, null, 2)}\n`,
  );
  await context.close();
  await browser.close();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
