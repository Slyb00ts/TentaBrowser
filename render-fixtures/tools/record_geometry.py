#!/usr/bin/env python3
"""Records getBoundingClientRect values from headless Chrome for HTML cases.

Usage: record_geometry.py replaced_cases.py GROUP out.rs

The cases module defines GROUP as a list of (name, (viewport width, height), html).
Every element with an `id` is recorded in document coordinates, after all images
have decoded, as a Rust `Case` table (see crates/tenta-render/tests/replaced.rs).
Needs Google Chrome; the path is CHROME below. HTML needs a <head> and a
closing </body> for the font declarations and the recording script.
"""
import json, os, re, subprocess, sys, tempfile, importlib.util, html as htmllib

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

# The bundled faces, declared so Chrome measures the same advances and metrics
# as the engine. Injected into <head> at record time; the stored HTML stays plain.
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
  const imgs = [...document.images];
  await Promise.all(imgs.map(i => i.decode().catch(() => {})));
  await new Promise(r => setTimeout(r, 50));
  const sx = window.scrollX, sy = window.scrollY;
  const out = {};
  for (const el of document.querySelectorAll('[id]')) {
    const r = el.getBoundingClientRect();
    out[el.id] = [r.x + sx, r.y + sy, r.width, r.height];
  }
  const de = document.documentElement;
  const result = {rects: out, scroll: [de.scrollWidth, de.scrollHeight], inner: [innerWidth, innerHeight]};
  document.documentElement.setAttribute('data-out', JSON.stringify(result));
})();
</script>
"""

def run(name, viewport, html, workdir, extra=(0, 0)):
    path = os.path.join(workdir, name + ".html")
    doc = html.replace("<head>", "<head>" + FONT_CSS, 1).replace("</body>", SCRIPT + "</body>")
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
    data = json.loads(htmllib.unescape(m.group(1)))
    return data

def fmt(v):
    v = round(v, 2)
    s = repr(float(v))
    return s

def main():
    spec = importlib.util.spec_from_file_location("cases", sys.argv[1])
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    group = getattr(mod, sys.argv[2])
    out_path = sys.argv[3]
    workdir = tempfile.mkdtemp(prefix="chromecases")
    lines = ["// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).\n",
             "\n", "use super::Case;\n", "\n", "pub(super) const CASES: &[Case] = &[\n"]
    for name, viewport, html in group:
        data = run(name, viewport, html, workdir)
        if tuple(data["inner"]) != tuple(viewport):
            extra = (viewport[0] - data["inner"][0], viewport[1] - data["inner"][1])
            data = run(name, viewport, html, workdir, extra)
        if data["inner"][0] != viewport[0]:
            print(f"warning: {name}: inner width {data['inner']} != viewport {viewport}", file=sys.stderr)
        assert '"##' not in html
        lines.append("    Case {\n")
        lines.append(f'        name: "{name}",\n')
        lines.append(f"        viewport: ({fmt(viewport[0])}, {fmt(viewport[1])}),\n")
        lines.append(f'        html: r##"{html}"##,\n')
        lines.append(f"        scroll: ({fmt(data['scroll'][0])}, {fmt(data['scroll'][1])}),\n")
        lines.append("        expect: &[\n")
        for el_id, rect in data["rects"].items():
            lines.append(f'            ("{el_id}", [{", ".join(fmt(x) for x in rect)}]),\n')
        lines.append("        ],\n    },\n")
        print(name, "ok", file=sys.stderr)
    lines.append("];\n")
    with open(out_path, "w") as f:
        f.writelines(lines)

main()
