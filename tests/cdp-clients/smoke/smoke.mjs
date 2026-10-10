// Drives a running `tentabrowser cdp` with stock Puppeteer.
//
//   cargo build -p tentabrowser-cli --features v8,render
//   target/debug/tentabrowser --allow-private-network --ephemeral cdp --port 9333 &
//   cd tests/cdp-clients/smoke && npm install && node server.mjs 8765 &
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

function expect(condition, message) {
  if (!condition) throw new Error(message);
}

// The Network and Fetch domains (D4b).
async function networkSteps(page, base) {
  const requests = [];
  const failures = [];
  page.on('request', (request) => requests.push(`${request.resourceType()} ${new URL(request.url()).pathname}`));
  page.on('requestfailed', (request) => failures.push(`${new URL(request.url()).pathname} ${request.failure()?.errorText}`));

  await step('goto returns the response', async () => {
    const response = await page.goto(`${base}/form.html`, { waitUntil: 'load' });
    expect(response.ok() && response.status() === 200, `status ${response.status()}`);
    expect(response.request().isNavigationRequest(), 'not a navigation request');
    expect(response.remoteAddress().ip === '127.0.0.1', JSON.stringify(response.remoteAddress()) + ' cache=' + response.fromCache());
    expect((await response.text()).includes('Fixture'), 'body');
    return response.headers()['content-type'];
  });
  await step('redirect chain', async () => {
    const response = await page.goto(`${base}/redirect`, { waitUntil: 'load' });
    const chain = response.request().redirectChain();
    expect(chain.length === 1 && chain[0].response().status() === 302, `chain ${chain.length}`);
    expect(new URL(response.url()).pathname === '/form.html', response.url());
    return chain.length;
  });
  await step('request events and waitForResponse', async () => {
    requests.length = 0;
    const image = page.waitForResponse((r) => r.url().endsWith('/pixel.png'));
    const api = page.waitForResponse((r) => r.url().endsWith('/api/data'));
    const xhr = page.waitForResponse((r) => r.url().endsWith('/api/data?x=1'));
    await page.goto(`${base}/net.html`, { waitUntil: 'load' });
    const [imageResponse, apiResponse, xhrResponse] = await Promise.all([image, api, xhr]);
    expect(imageResponse.status() === 200, 'image status');
    expect((await imageResponse.buffer()).length === 70, 'image body');
    expect((await apiResponse.json()).hello === 'world', 'fetch body');
    expect((await xhrResponse.json()).hello === 'world', 'xhr body');
    expect(apiResponse.request().resourceType() === 'fetch', apiResponse.request().resourceType());
    expect(xhrResponse.request().resourceType() === 'xhr', xhrResponse.request().resourceType());
    return requests.slice().sort();
  });
  await step('setExtraHTTPHeaders and setUserAgent', async () => {
    await page.setExtraHTTPHeaders({ 'x-smoke': 'yes' });
    await page.setUserAgent('SmokeAgent/1');
    await page.goto(`${base}/echo`, { waitUntil: 'load' });
    const sent = JSON.parse(await page.$eval('#h', (e) => e.textContent));
    expect(sent['x-smoke'] === 'yes', 'extra header');
    expect(sent['user-agent'] === 'SmokeAgent/1', sent['user-agent']);
    await page.setExtraHTTPHeaders({});
    return 'reached the server';
  });
  await step('cookies', async () => {
    await page.goto(`${base}/cookie`, { waitUntil: 'load' });
    expect((await page.cookies()).some((c) => c.name === 'smoke' && c.value === '1'), 'cookie from the server');
    await page.setCookie({ name: 'mine', value: '2' });
    await page.goto(`${base}/echo`, { waitUntil: 'load' });
    const sent = JSON.parse(await page.$eval('#h', (e) => e.textContent));
    expect(sent.cookie.includes('mine=2') && sent.cookie.includes('smoke=1'), sent.cookie);
    await page.deleteCookie({ name: 'mine' });
    expect(!(await page.cookies()).some((c) => c.name === 'mine'), 'deleted');
    return sent.cookie;
  });
  await step('request interception', async () => {
    await page.setRequestInterception(true);
    const handler = (request) => {
      const path = new URL(request.url()).pathname;
      if (path === '/api/data') {
        return request.respond({ status: 200, contentType: 'application/json', body: '{"intercepted":true}' });
      }
      if (path === '/pixel.png') return request.abort();
      return request.continue();
    };
    page.on('request', handler);
    failures.length = 0;
    const failed = page.waitForRequest((r) => r.url().endsWith('/pixel.png'));
    await page.goto(`${base}/net.html`, { waitUntil: 'load' });
    await failed;
    await page.waitForFunction(() => window.apiResult && window.xhrResult, { timeout: 5000 });
    expect(JSON.parse(await page.evaluate(() => window.apiResult)).intercepted === true, 'fetch was answered by the client');
    expect(JSON.parse(await page.evaluate(() => window.xhrResult)).intercepted === true, 'xhr was answered by the client');
    await page.waitForFunction(() => true);
    page.off('request', handler);
    await page.setRequestInterception(false);
    return failures;
  });
  await step('respond with a redirect', async () => {
    await page.setRequestInterception(true);
    const handler = (request) => {
      if (new URL(request.url()).pathname === '/fake') {
        return request.respond({ status: 302, headers: { location: '/form.html', 'set-cookie': 'faked=1; Path=/' } });
      }
      return request.continue();
    };
    page.on('request', handler);
    const response = await page.goto(`${base}/fake`, { waitUntil: 'load' });
    page.off('request', handler);
    await page.setRequestInterception(false);
    expect(new URL(response.url()).pathname === '/form.html', response.url());
    expect(response.request().redirectChain().length === 1, 'redirect chain');
    expect((await page.cookies()).some((c) => c.name === 'faked'), 'the faked Set-Cookie was stored');
    return response.status();
  });
  await step('offline mode', async () => {
    await page.setOfflineMode(true);
    let error = '';
    try {
      await page.goto(`${base}/form.html`, { waitUntil: 'load' });
    } catch (caught) {
      error = String(caught.message);
    }
    await page.setOfflineMode(false);
    expect(error.includes('ERR_INTERNET_DISCONNECTED'), error);
    return error;
  });
  await step('setCacheEnabled', async () => {
    await page.setCacheEnabled(false);
    const response = await page.goto(`${base}/form.html`, { waitUntil: 'load' });
    await page.setCacheEnabled(true);
    return response.status();
  });
}

const browser = await puppeteer.connect({ browserWSEndpoint: endpoint, protocolTimeout: 20000 });
const page = await step('newPage', () => browser.newPage());
if (page) {
  await step('setViewport', () => page.setViewport({ width: 640, height: 480, deviceScaleFactor: 1 }));
  await step('goto', async () => {
    const response = await page.goto(url, { waitUntil: 'load' });
    if (!response) throw new Error('page.goto resolved null');
    if (!response.headers()['content-type']?.startsWith('text/html')) throw new Error('no content-type');
    return response.status();
  });
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
  await networkSteps(page, new URL(url).origin);
  await step('close', () => page.close());
}
await browser.disconnect();
process.exit(failed ? 1 : 0);
