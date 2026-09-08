#!/usr/bin/env python3
"""Generates the static-article fixture: index.html (~1 MB), placeholder
images and expected.txt. Deterministic (seeded); stdlib only.

    python3 tests/fixtures/static-article/generate.py
"""
import os
import random
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
TARGET_BYTES = 1_000_000
SENTINEL = "END-OF-ARTICLE-7F3A"

WORDS = (
    "the of and to in a is that for it as was with be by on not he this are "
    "or his from at which but have an had they you were their one all we can "
    "her has there been if more when will would who so no she other its may "
    "these than then now some two out into over time such only any new first "
    "after where most through between under while before also during without "
    "because whether although engine parser layout render network protocol "
    "document element attribute stylesheet selector cascade inline block table "
    "column row header footer paragraph section article figure caption image "
    "anchor link script settle budget latency throughput memory arena atom "
    "thread isolate snapshot fixture harness baseline scenario measure report "
    "signal channel stream buffer socket packet frame codec sample quantum "
    "platform desktop mobile device kernel process page tier policy profile "
    "cache pool queue timer interval callback promise event listener target "
    "origin cookie session storage index record field value string number "
    "boolean object array function method property prototype module bundle "
    "compile optimise allocate release recycle measure compare interleave"
).split()

CAPITALISED = ["Rust", "Chromium", "Servo", "Ladybird", "Stylo", "WebKit", "Gecko"]


def sentence(rng):
    n = rng.randint(6, 18)
    words = [rng.choice(WORDS) for _ in range(n)]
    if rng.random() < 0.15:
        words[rng.randrange(n)] = rng.choice(CAPITALISED)
    words[0] = words[0].capitalize()
    if rng.random() < 0.2:
        cut = rng.randint(2, n - 2)
        words[cut] += ","
    return " ".join(words) + rng.choice([".", ".", ".", "?", "!"])


def inline_markup(rng, text):
    """Adds a link, emphasis or code span to some sentences (HTML only)."""
    roll = rng.random()
    if roll < 0.08:
        return f'<a href="/static-article/index.html#s{rng.randint(1, 60)}">{text}</a>'
    if roll < 0.14:
        return f"<em>{text}</em>"
    if roll < 0.18:
        return f"<strong>{text}</strong>"
    if roll < 0.21:
        return f"<code>{rng.choice(WORDS)}_{rng.randint(0, 99)}</code> {text}"
    return text


def paragraph(rng):
    sentences = [sentence(rng) for _ in range(rng.randint(4, 9))]
    html = " ".join(inline_markup(rng, s) for s in sentences)
    return html, " ".join(strip_tags(s) for s in [html])


def strip_tags(html):
    out, depth = [], 0
    for ch in html:
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
        elif depth == 0:
            out.append(ch)
    return " ".join("".join(out).split())


STYLE = """
:root { --ink: #1c1c1c; --muted: #5a5a5a; --rule: #d9d9d9; --accent: #0a4d8c; }
html { font: 17px/1.55 Georgia, "Times New Roman", serif; color: var(--ink); background: #fbfaf7; }
body { margin: 0; }
header.site { border-bottom: 1px solid var(--rule); padding: 12px 24px; display: flex; justify-content: space-between; }
header.site nav a { margin-left: 16px; color: var(--accent); text-decoration: none; }
main { max-width: 760px; margin: 0 auto; padding: 24px; }
h1 { font-size: 2.2rem; line-height: 1.15; margin: 0.6em 0 0.3em; }
h2 { font-size: 1.5rem; margin-top: 2em; border-bottom: 1px solid var(--rule); padding-bottom: 4px; }
h3 { font-size: 1.15rem; margin-top: 1.4em; color: var(--muted); }
p { margin: 0 0 1em; text-align: justify; hyphens: auto; }
p.byline { color: var(--muted); font-style: italic; }
a { color: var(--accent); }
blockquote { margin: 1.2em 0; padding: 0.4em 1.2em; border-left: 4px solid var(--accent); background: #f1efe9; }
figure { margin: 1.5em 0; text-align: center; }
figure img { max-width: 100%; height: auto; border: 1px solid var(--rule); }
figcaption { font-size: 0.85rem; color: var(--muted); margin-top: 6px; }
table { border-collapse: collapse; width: 100%; font-size: 0.9rem; margin: 1.5em 0; }
th, td { border: 1px solid var(--rule); padding: 6px 10px; text-align: left; }
th { background: #ece9e1; }
tbody tr:nth-child(even) { background: #f6f4ef; }
ul.toc { columns: 2; font-size: 0.9rem; }
ul.toc li { break-inside: avoid; }
code { font: 0.9em/1 Menlo, Consolas, monospace; background: #ece9e1; padding: 1px 4px; border-radius: 3px; }
footer.site { border-top: 1px solid var(--rule); margin-top: 3em; padding: 16px 24px; font-size: 0.85rem; color: var(--muted); }
@media (max-width: 600px) { html { font-size: 15px; } main { padding: 12px; } ul.toc { columns: 1; } }
""".strip()


def png(width, height, rgb):
    """A solid-colour, non-interlaced 8-bit RGB PNG (stdlib only)."""
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main():
    rng = random.Random(0x7F3A)
    html = []
    text = []

    def block(tag_html, plain):
        html.append(tag_html)
        if plain:
            text.append(plain)

    nav_links = [("Home", "/"), ("Articles", "/static-article/"), ("About", "/tiny/"), ("Contact", "/api/echo")]
    html.append("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n")
    html.append("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n")
    html.append("<title>The Long Read: A Field Guide to Headless Engines</title>\n")
    html.append(f"<style>\n{STYLE}\n</style>\n</head>\n<body>\n")
    html.append("<header class=\"site\"><span class=\"brand\">Static Article Fixture</span><nav>")
    for label, href in nav_links:
        html.append(f'<a href="{href}">{label}</a>')
        text.append(label)
    html.append("</nav></header>\n<main>\n<article>\n")
    block("<h1>The Long Read: A Field Guide to Headless Engines</h1>", "The Long Read: A Field Guide to Headless Engines")
    block('<p class="byline">By the fixture generator, published 2026-09-06, 1 MB of deterministic prose.</p>',
          "By the fixture generator, published 2026-09-06, 1 MB of deterministic prose.")

    toc_marker = len(html)
    html.append("")  # filled with the table of contents once sections are known
    toc_text_marker = len(text)
    text.append("")

    figures_used = 0
    table_written = False
    section = 0
    size = sum(len(h.encode()) for h in html)
    while size < TARGET_BYTES:
        section += 1
        title = sentence(rng).rstrip(".?!")
        block(f'<h2 id="s{section}">{section}. {title}</h2>', f"{section}. {title}")
        for sub in range(2):
            sub_title = sentence(rng).rstrip(".?!")
            block(f"<h3>{section}.{sub + 1} {sub_title}</h3>", f"{section}.{sub + 1} {sub_title}")
            for _ in range(rng.randint(4, 7)):
                p_html, _ = paragraph(rng)
                block(f"<p>{p_html}</p>", strip_tags(p_html))
            if rng.random() < 0.5:
                items = [sentence(rng) for _ in range(rng.randint(3, 6))]
                html.append("<ul>" + "".join(f"<li>{i}</li>" for i in items) + "</ul>")
                text.extend(items)
        if section % 4 == 0:
            quote = sentence(rng) + " " + sentence(rng)
            block(f"<blockquote><p>{quote}</p></blockquote>", quote)
        if section % 5 == 0 and figures_used < 12:
            figures_used += 1
            caption = f"Figure {figures_used}: {sentence(rng)}"
            block(
                f'<figure><img src="img/figure-{figures_used}.png" alt="Placeholder figure {figures_used}" '
                f'width="640" height="360"><figcaption>{caption}</figcaption></figure>',
                caption,
            )
        if section == 20 and not table_written:
            table_written = True
            heads = ["Scenario", "Subject", "p50 (ms)", "p95 (ms)", "Peak RSS (MB)"]
            rows = []
            for i in range(40):
                rows.append([
                    f"S-{rng.choice(WORDS)}-{i}", rng.choice(["reference", "chromium", "tentabrowser"]),
                    f"{rng.uniform(1, 900):.1f}", f"{rng.uniform(1, 1200):.1f}", f"{rng.uniform(5, 400):.0f}",
                ])
            html.append("<table><caption>Table 1: illustrative measurements</caption><thead><tr>"
                        + "".join(f"<th>{h}</th>" for h in heads) + "</tr></thead><tbody>"
                        + "".join("<tr>" + "".join(f"<td>{c}</td>" for c in r) + "</tr>" for r in rows)
                        + "</tbody></table>")
            text.append("Table 1: illustrative measurements")
            text.append("\t".join(heads))
            text.extend("\t".join(r) for r in rows)
        size = sum(len(h.encode()) for h in html)

    closing = f"This is the last paragraph of the article. Marker: {SENTINEL}."
    block(f"<p>{closing}</p>", closing)
    html.append("</article>\n</main>\n")
    footer_links = [("Fixture index", "/"), ("Generator", "generate.py"), ("Expected text", "expected.txt")]
    html.append('<footer class="site"><p>Generated fixture, Apache-2.0. ' + " · ".join(
        f'<a href="{href}">{label}</a>' for label, href in footer_links) + "</p></footer>\n</body>\n</html>\n")
    text.append("Generated fixture, Apache-2.0. " + " · ".join(label for label, _ in footer_links))

    toc = '<ul class="toc">' + "".join(
        f'<li><a href="#s{i}">Section {i}</a></li>' for i in range(1, section + 1)) + "</ul>"
    html[toc_marker] = "<h2>Contents</h2>" + toc
    text[toc_text_marker] = "Contents\n" + "\n".join(f"Section {i}" for i in range(1, section + 1))

    with open(os.path.join(HERE, "index.html"), "w", encoding="utf-8", newline="\n") as f:
        f.write("".join(html))
    with open(os.path.join(HERE, "expected.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(text) + "\n")
    palette = [(0x0A, 0x4D, 0x8C), (0x8C, 0x2F, 0x0A), (0x2E, 0x7D, 0x32), (0x6A, 0x1B, 0x9A),
               (0xC7, 0x8A, 0x00), (0x00, 0x69, 0x7A), (0x5D, 0x40, 0x37), (0x37, 0x47, 0x4F),
               (0xAD, 0x14, 0x57), (0x1B, 0x5E, 0x20), (0xE6, 0x51, 0x00), (0x28, 0x35, 0x93)]
    for i in range(1, figures_used + 1):
        with open(os.path.join(HERE, "img", f"figure-{i}.png"), "wb") as f:
            f.write(png(64, 36, palette[(i - 1) % len(palette)]))
    total = sum(len(h.encode()) for h in html)
    print(f"index.html: {total} bytes, {section} sections, {figures_used} figures, "
          f"{len(text)} text blocks; expected.txt written")


if __name__ == "__main__":
    main()
