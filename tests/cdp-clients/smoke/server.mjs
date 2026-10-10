// Fixture site for smoke.mjs: the form page, a page with subresources, and
// endpoints that echo what the browser sent.
//
//   node server.mjs [port]      # prints "listening http://127.0.0.1:<port>"
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
// A 1x1 PNG.
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);
const NET_PAGE = `<!doctype html><html><head><title>Net</title></head><body>
<img src="/pixel.png" width="1" height="1"><div id="out">idle</div>
<script>
fetch('/api/data').then(function (r) { return r.json(); }).then(function (j) { window.apiResult = JSON.stringify(j); });
var x = new XMLHttpRequest(); x.open('GET', '/api/data?x=1');
x.onload = function () { window.xhrResult = x.responseText; }; x.send();
</script></body></html>`;

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  const send = (status, type, body, headers = {}) => {
    res.writeHead(status, { 'Content-Type': type, 'Content-Length': Buffer.byteLength(body), ...headers });
    res.end(body);
  };
  switch (url.pathname) {
    case '/form.html':
      return send(200, 'text/html; charset=utf-8', fs.readFileSync(path.join(here, 'form.html')));
    case '/net.html':
      return send(200, 'text/html; charset=utf-8', NET_PAGE);
    case '/pixel.png':
      return send(200, 'image/png', PIXEL);
    case '/api/data':
      return send(200, 'application/json', JSON.stringify({ hello: 'world' }));
    case '/redirect':
      return send(302, 'text/plain', '', { Location: '/form.html' });
    case '/cookie':
      return send(200, 'text/html', '<!doctype html><title>cookie</title>', {
        'Set-Cookie': 'smoke=1; Path=/',
      });
    case '/echo':
      return send(200, 'text/html', `<!doctype html><title>echo</title><pre id="h">${JSON.stringify(req.headers)}</pre>`);
    default:
      return send(404, 'text/plain', 'not found');
  }
});
server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(`listening http://127.0.0.1:${server.address().port}`);
});
