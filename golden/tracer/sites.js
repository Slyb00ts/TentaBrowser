// Golden-site scenarios for the API tracer. Each site exports `title`,
// `urls`, optional `limitations`, and `run(h)` driving the page through the
// harness helpers (goto, settle, consent, scroll, harvest, note, ...).
//
// Page-side evaluations wrap their body in `tracer.quiet(...)` so that the
// harness's own DOM reads are not counted as site API usage.
'use strict';

const QUERY = 'rust+browser+engine';

const CONSENT_SELECTORS = [
  '#L2AGLb',                                             // google.com "Accept all"
  'button[aria-label="Accept all"]',
  'button[aria-label*="Accept the use of cookies"]',     // youtube.com consent dialog
  'ytd-button-renderer:has-text("Accept all") button',
  'tp-yt-paper-button:has-text("Accept all")',
  'form[action*="consent"] button:has-text("Accept all")',
  'form[action*="consent"] button:has-text("Zaakceptuj wszystko")',
  'button:has-text("Accept all")',
  'button:has-text("I agree")',
  '#introAgreeButton',
];

const QUIET = `const tracer = globalThis[Symbol.for('tenta.tracer')];`;

async function countResults(h, selectors) {
  return h.page.evaluate((sels) => {
    const tracer = globalThis[Symbol.for('tenta.tracer')];
    const f = () => {
      const out = {};
      for (const s of sels) out[s] = document.querySelectorAll(s).length;
      out.title = document.title;
      out.text_sample = (document.body && document.body.innerText || '').slice(0, 300).replace(/\s+/g, ' ');
      return out;
    };
    return tracer ? tracer.quiet(f) : f();
  }, selectors).catch((e) => ({ error: e.message }));
}

module.exports = {
  ddg: {
    title: 'DuckDuckGo',
    urls: [`https://duckduckgo.com/?q=${QUERY}`, `https://html.duckduckgo.com/html/?q=${QUERY}`],
    limitations: ['Only the results page for one query and the no-JS HTML endpoint were exercised; no pagination, image or news verticals.'],
    async run(h) {
      await h.goto(h.inv.meta.urls[0]);
      await h.settle(10000);
      await h.scroll();
      await h.settle(3000);
      h.note(`results: ${JSON.stringify(await countResults(h, ['[data-testid="result"]', 'article', 'h2 a']))}`);
      await h.screenshot('js');
      await h.harvest('ddg-js');
      await h.goto(h.inv.meta.urls[1]);
      await h.settle(5000);
      h.note(`html results: ${JSON.stringify(await countResults(h, ['.result', '.result__a', 'form']))}`);
      await h.screenshot('html');
      await h.harvest('ddg-html');
    },
  },

  google: {
    title: 'Google Search',
    urls: [`https://www.google.com/search?q=${QUERY}&hl=en`],
    limitations: ['One query on the desktop SERP after accepting the consent interstitial; if Google served a block or CAPTCHA the inventory covers only what loaded (see meta.notes).'],
    async run(h) {
      await h.goto(h.inv.meta.urls[0]);
      const consented = await h.consent(CONSENT_SELECTORS);
      h.note(`consent interstitial: ${consented ? 'accepted' : 'not shown or not matched'}`);
      await h.settle(10000);
      const url = h.page.url();
      const probe = await countResults(h, ['#search a h3', '#rso', '#captcha-form', 'iframe[src*="recaptcha"]', 'form#captcha-form']);
      const blocked = /\/sorry\//.test(url) || (probe['#captcha-form'] > 0) || /unusual traffic/i.test(probe.text_sample || '');
      h.note(`google page: ${JSON.stringify(probe)}; blocked=${blocked}`);
      if (blocked) h.limitation(`Google served a block/CAPTCHA page (${url}); the inventory reflects the interstitial and the block page only.`);
      await h.scroll();
      await h.settle(3000);
      await h.screenshot('serp');
      await h.harvest('google-serp');
    },
  },

  youtube: {
    title: 'YouTube',
    urls: ['https://www.youtube.com/', 'https://www.youtube.com/watch?v=aqz-KE-bpKQ'],
    limitations: ['Home page plus one non-DRM video (Big Buck Bunny) played muted for 20 s; no seek, quality change, comments, search or sign-in flows. Chromium (no proprietary codecs) is served VP9/AV1 + Opus, so H.264/AAC MSE paths are not observed.'],
    async run(h) {
      await h.goto(h.inv.meta.urls[0]);
      const consented = await h.consent(CONSENT_SELECTORS);
      h.note(`consent interstitial: ${consented ? 'accepted' : 'not shown or not matched'}`);
      await h.settle(10000);
      await h.scroll(2);
      await h.screenshot('home');
      await h.harvest('youtube-home');

      await h.goto(h.inv.meta.urls[1]);
      if (await h.consent(CONSENT_SELECTORS)) h.note('consent interstitial accepted on the watch page');
      await h.settle(8000);
      const videoState = () => h.page.evaluate(() => {
        const tracer = globalThis[Symbol.for('tenta.tracer')];
        const f = () => {
          const v = document.querySelector('video');
          if (!v) return null;
          const q = v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality() : null;
          return { paused: v.paused, currentTime: v.currentTime, readyState: v.readyState, muted: v.muted,
            videoWidth: v.videoWidth, videoHeight: v.videoHeight, src_scheme: (v.currentSrc || v.src || '').split(':')[0],
            dropped: q && q.droppedVideoFrames, total_frames: q && q.totalVideoFrames, error: v.error && v.error.code };
        };
        return tracer ? tracer.quiet(f) : f();
      }).catch(() => null);
      await h.page.waitForFunction(() => {
        const tracer = globalThis[Symbol.for('tenta.tracer')];
        const f = () => { const v = document.querySelector('video'); return !!v && v.readyState >= 1; };
        return tracer ? tracer.quiet(f) : f();
      }, null, { timeout: 30000 }).catch(() => h.note('no <video> with metadata within 30 s'));
      await h.page.evaluate(() => {
        const tracer = globalThis[Symbol.for('tenta.tracer')];
        const f = () => { const v = document.querySelector('video'); if (v) { v.muted = true; const p = v.play(); if (p && p.catch) p.catch(() => {}); } };
        return tracer ? tracer.quiet(f) : f();
      }).catch(() => {});
      const waitPlaying = () => h.page.waitForFunction(() => {
        const tracer = globalThis[Symbol.for('tenta.tracer')];
        const f = () => { const v = document.querySelector('video'); return !!v && !v.paused && v.currentTime > 1; };
        return tracer ? tracer.quiet(f) : f();
      }, null, { timeout: 20000 }).then(() => true).catch(() => false);
      let playing = await waitPlaying();
      if (!playing) {
        for (const sel of ['.ytp-large-play-button', 'button.ytp-play-button', '#movie_player']) {
          const loc = h.page.locator(sel).first();
          if (await loc.isVisible().catch(() => false)) { await loc.click({ timeout: 5000 }).catch(() => {}); h.note(`clicked ${sel}`); break; }
        }
        playing = await waitPlaying();
      }
      h.note(`playback started: ${playing}; state ${JSON.stringify(await videoState())}`);
      if (!playing) h.limitation('Playback did not start; the MSE/media interface counts reflect setup only.');
      await h.sleep(20000);
      h.note(`after 20 s: ${JSON.stringify(await videoState())}`);
      await h.screenshot('watch');
      await h.harvest('youtube-watch');
    },
  },

  teams: {
    title: 'Microsoft Teams (pre-auth)',
    urls: ['https://teams.microsoft.com/'],
    limitations: [
      'The authenticated Teams client is NOT covered: teams.microsoft.com redirects to the Microsoft login page and no test tenant credentials were available. Only the pre-auth shell and the login page were captured. The full client (calling, chat, Web Audio worklets, WebRTC, Service Worker, IndexedDB) must be captured with a test tenant later.',
    ],
    async run(h) {
      await h.goto(h.inv.meta.urls[0], { waitUntil: 'load', timeout: 60000 });
      // Follow the redirect chain (teams shell -> login.microsoftonline.com or go.microsoft.com) until the URL stops changing.
      let last = h.page.url();
      for (let i = 0; i < 12; i++) {
        await h.sleep(2500);
        const now = h.page.url();
        if (now !== last) { h.note(`redirected to ${now}`); last = now; continue; }
        break;
      }
      await h.settle(15000);
      h.note(`landing page: ${JSON.stringify(await countResults(h, ['input[type="email"]', 'input[name="loginfmt"]', 'form', 'iframe']))}`);
      await h.screenshot('landing');
      await h.harvest('teams-landing');
    },
  },

  genericRun(url) {
    return async (h) => {
      await h.goto(url);
      await h.settle(8000);
      await h.scroll(2);
      await h.settle(2000);
      await h.screenshot('page');
      await h.harvest('page');
    };
  },
};
