#!/usr/bin/env python3
"""Records script-visible geometry and getComputedStyle values from headless Chrome.

Usage: record_script_api.py script_api_cases.py OUT.json [NAME ...]

For every case of GEOMETRY and STYLE in the cases module, loads the page in Chrome
(reference only), waits for fonts and images, calls the page's `collect()` and
stores `{name, viewport, html, expect}`. `crates/tenta-browser/tests/script_api_chrome.rs`
loads the same html in this engine and compares its `collect()` with `expect`.
Needs Google Chrome; the path is CHROME below.
"""
import html as htmllib, importlib.util, json, os, re, subprocess, sys, tempfile

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
FONTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "tenta-layout", "fonts")
FONT_CSS = """<style>
@font-face{font-family:'Liberation Sans';font-weight:400;src:url('file://%s/LiberationSans-Regular.ttf')}
@font-face{font-family:'Liberation Sans';font-weight:700;src:url('file://%s/LiberationSans-Bold.ttf')}
@font-face{font-family:'Liberation Serif';font-weight:400;src:url('file://%s/LiberationSerif-Regular.ttf')}
</style>""" % ((os.path.normpath(FONTS),) * 3)

SCRIPT = """
<script>
(async () => {
  try { await Promise.all([document.fonts.load("16px 'Liberation Sans'"), document.fonts.load("bold 16px 'Liberation Sans'"), document.fonts.load("16px 'Liberation Serif'")]); } catch (e) {}
  await document.fonts.ready;
  await new Promise(r => setTimeout(r, 50));
  let out;
  try { out = {inner: [innerWidth, innerHeight], expect: collect()}; }
  catch (e) { out = {inner: [innerWidth, innerHeight], error: String(e)}; }
  document.documentElement.setAttribute('data-out', JSON.stringify(out));
})();
</script>
"""

def run(name, viewport, html, workdir, extra=(0, 0)):
    path = os.path.join(workdir, name + ".html")
    head = html.index("<head>") + len("<head>") if "<head>" in html else None
    doc = html if head is None else html[:head] + FONT_CSS + html[head:]
    doc = doc.replace("</body>", SCRIPT + "</body>")
    with open(path, "w") as f:
        f.write(doc)
    cmd = [CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
           "--force-device-scale-factor=1",
           f"--window-size={viewport[0]+extra[0]},{viewport[1]+extra[1]}", "--virtual-time-budget=4000",
           "--allow-file-access-from-files", "--dump-dom", "file://" + path]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=60).stdout
    m = re.search(r'data-out="([^"]*)"', out)
    if not m:
        raise SystemExit(f"{name}: no output\n{out[:500]}")
    return json.loads(htmllib.unescape(m.group(1)))

def main():
    spec = importlib.util.spec_from_file_location("cases", sys.argv[1])
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    only = set(sys.argv[3:])
    workdir = tempfile.mkdtemp(prefix="scriptapi")
    cases = []
    for name, viewport, html in mod.GEOMETRY + mod.STYLE:
        if only and name not in only:
            continue
        data = run(name, viewport, html, workdir)
        if tuple(data["inner"]) != tuple(viewport):
            extra = (viewport[0] - data["inner"][0], viewport[1] - data["inner"][1])
            data = run(name, viewport, html, workdir, extra)
        if tuple(data["inner"]) != tuple(viewport):
            print(f"warning: {name}: inner {data['inner']} != viewport {viewport}", file=sys.stderr)
        if "error" in data:
            raise SystemExit(f"{name}: collect() threw {data['error']}")
        cases.append({"name": name, "viewport": list(viewport), "html": html, "expect": data["expect"]})
        print(name, "ok", file=sys.stderr)
    with open(sys.argv[2], "w") as f:
        json.dump(cases, f, indent=1, sort_keys=True)
        f.write("\n")

main()
