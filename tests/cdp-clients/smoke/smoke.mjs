// Drives a running `tentabrowser cdp` with stock Puppeteer.
//
//   cargo build -p tentabrowser-cli --features v8,render
//   target/debug/tentabrowser --allow-private-network --ephemeral cdp --port 9333 &
//   (cd tests/cdp-clients/smoke && python3 -m http.server 8765 &)
//   cd tests/cdp-clients/smoke && npm install
//   node smoke.mjs ws://127.0.0.1:9333/devtools/browser/<id> http://127.0.0.1:8765/form.html out.png
//
// The browser id is on the "DevTools listening on" line the server prints.
// Exits 1 when a step fails.
import fs from 'node:fs';
import puppeteer from 'puppeteer-core';

const [endpoint, url, out] = process.argv.slice(2);
if (!endpoint || !url || !out) {
  console.error('usage: node smoke.mjs <browserWSEndpoint> <form url> <screenshot.png>');
  process.exit(2);
}

let failed = false;
async function step(name, run) {
  try {
    const result = await run();
    console.log('OK  ', name, result === undefined ? '' : JSON.stringify(result));
    return result;
  } catch (error) {
    failed = true;
    console.log('FAIL', name, String((error && error.message) || error).split('\n')[0]);
  }
}

const browser = await puppeteer.connect({ browserWSEndpoint: endpoint, protocolTimeout: 20000 });
const page = await step('newPage', () => browser.newPage());
if (page) {
  await step('setViewport', () => page.setViewport({ width: 640, height: 480, deviceScaleFactor: 1 }));
  // The response object needs Network events (D4b), so it may be null.
  await step('goto', async () => (await page.goto(url, { waitUntil: 'load' }))?.status() ?? 'no response object');
  await step('$', async () => !!(await page.$('#name')));
  await step('waitForSelector', async () => !!(await page.waitForSelector('#late', { timeout: 5000 })));
  await step('type', () => page.type('#name', 'Ada'));
  await step('value', () => page.$eval('#name', (e) => e.value));
  await step('click', () => page.click('#go'));
  await step('waitForFunction', () =>
    page.waitForFunction(() => document.getElementById('out').textContent.startsWith('Hello'), { timeout: 5000 }),
  );
  await step('text', () => page.$eval('#out', (e) => e.textContent));
  await step('content', async () => (await page.content()).includes('Hello, Ada!'));
  await step('evaluate', () => page.evaluate(() => 6 * 7));
  await step('screenshot', async () => {
    await page.screenshot({ path: out });
    return fs.statSync(out).size;
  });
  await step('close', () => page.close());
}
await browser.disconnect();
process.exit(failed ? 1 : 0);
