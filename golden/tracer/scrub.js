// Removes incidental identifiers from an inventory's `meta` before it is
// committed: public IP addresses echoed by block pages, and the query strings
// of long redirect URLs (OAuth nonces, session ids). Applied by tracer.js at
// write time; can be re-applied to an existing file (idempotent).
'use strict';

const IPV4 = /\b(?:\d{1,3}\.){3}\d{1,3}\b/g;
const LONG_URL = /https?:\/\/[^\s"')]{160,}/g;

function stripQuery(url) {
  try { const u = new URL(url); return u.origin + u.pathname + (u.search ? '?<stripped>' : ''); } catch (_) { return url.slice(0, 160); }
}

function scrubText(s) {
  return String(s).replace(IPV4, '<ip>').replace(LONG_URL, (m) => stripQuery(m)).slice(0, 800);
}

function scrubMeta(meta) {
  meta.notes = (meta.notes || []).map(scrubText);
  meta.limitations = (meta.limitations || []).map(scrubText);
  meta.navigations = (meta.navigations || []).map((n) => ({ ...n, url: n.url.length > 160 ? stripQuery(n.url) : n.url }));
  meta.steps = (meta.steps || []).map((s) => ({ ...s, url: s.url && s.url.length > 160 ? stripQuery(s.url) : s.url }));
  if (meta.final_url && meta.final_url.length > 160) meta.final_url = stripQuery(meta.final_url);
  if (meta.page_title) meta.page_title = scrubText(meta.page_title);
  meta.page_errors.samples = (meta.page_errors.samples || []).map(scrubText);
  return meta;
}

module.exports = { scrubMeta, scrubText };
