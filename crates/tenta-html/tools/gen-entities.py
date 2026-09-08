#!/usr/bin/env python3
"""Regenerate `src/tokenizer/entities.rs` from the WHATWG entity table.

The table is the one the HTML standard publishes at
<https://html.spec.whatwg.org/entities.json> and that the `entities` npm
package redistributes. Run from anywhere:

    python3 crates/tenta-html/tools/gen-entities.py            # download
    python3 crates/tenta-html/tools/gen-entities.py entities.json

The generated table is sorted by name so the tokenizer can narrow the
candidate range with a binary search on one byte at a time; the leading `&` is
not stored because the tokenizer has already consumed it.
"""

import json
import pathlib
import sys
import urllib.request

SOURCE = "https://html.spec.whatwg.org/entities.json"
OUTPUT = pathlib.Path(__file__).resolve().parent.parent / "src/tokenizer/entities.rs"

HEADER = """// Generated table: do not edit by hand.
//
// Source data: the WHATWG named character reference table
// (<https://html.spec.whatwg.org/entities.json>), as redistributed by the
// `entities` crate (MIT, Philip Jackson). Regenerate with
// `crates/tenta-html/tools/gen-entities.py`.

/// Named character references, sorted by name (the leading `&` is not
/// stored). `1` and `2` are the code points; `2` is `0` for single-character
/// references.
pub(crate) static NAMED_ENTITIES: &[(&str, u32, u32)] = &[
"""


def main() -> int:
    if len(sys.argv) > 1:
        data = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    else:
        with urllib.request.urlopen(SOURCE) as response:
            data = json.loads(response.read().decode("utf-8"))

    rows = []
    for name, entry in data.items():
        assert name.startswith("&"), name
        points = entry["codepoints"]
        assert 1 <= len(points) <= 2, (name, points)
        first = points[0]
        second = points[1] if len(points) == 2 else 0
        rows.append((name[1:], first, second))
    rows.sort(key=lambda row: row[0].encode("utf-8"))

    longest = max(len(name) for name, _, _ in rows)
    lines = [HEADER]
    for name, first, second in rows:
        lines.append(f'    ("{name}", {first}, {second}),\n')
    lines.append("];\n\n")
    lines.append("/// Length in bytes of the longest name in the table.\n")
    lines.append(f"pub(crate) const MAX_ENTITY_NAME_LEN: usize = {longest};\n")

    OUTPUT.write_text("".join(lines), encoding="utf-8")
    print(f"wrote {OUTPUT} with {len(rows)} entities (longest name {longest} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
