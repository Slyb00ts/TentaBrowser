# static-article

A ~1 MB realistic static article: 153 numbered sections with `h2`/`h3`
headings, roughly 1 700 paragraphs of seeded pseudo-prose with inline links,
emphasis and code spans, bullet lists, block quotes, a 40-row × 5-column
table, twelve `<figure>` images referencing local placeholder PNGs in `img/`,
a table of contents, header navigation and a footer. One inline stylesheet,
**no JavaScript**, no external resources.

Used by `S-static-text`, `S-static-text-cli`, `S-screenshot`, `S-fullpage`
and `S-concurrency-t1` (`docs/spec/15-testing-and-benchmarks.md` §2).

## Regenerating

```bash
python3 tests/fixtures/static-article/generate.py
```

The generator is deterministic (seed `0x7F3A`, stdlib only) and rewrites
`index.html`, `expected.txt` and `img/figure-*.png`. Do not hand-edit the
generated files.

## Expected settled text (`expected.txt`)

One line per block-level element in document order (headings, paragraphs,
list items, block quotes, figure captions, navigation and footer link text);
table rows as one line per row with cells separated by a tab; inline
whitespace collapsed. Engines format `--dump text` differently, so the
harness only checks that the output contains the sentinel of the final
paragraph, `END-OF-ARTICLE-7F3A`; `expected.txt` is the reference for
extraction tests that compare normalised block text.
