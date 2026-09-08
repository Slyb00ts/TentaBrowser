# WPT expectations

Expectation files record the results a product is known to produce so that
`wpt/run.py` can flag regressions (unexpected failures) and improvements
(unexpected passes, which should be turned into tightened expectations).

Layout: `wpt/expectations/<product>/<wpt path>.ini`, one file per test file,
mirroring the WPT tree. Example: the expectations for
`/dom/nodes/Node-appendChild.html` on `tentabrowser` live in
`wpt/expectations/tentabrowser/dom/nodes/Node-appendChild.html.ini`.

Format (a subset of wptrunner's metadata syntax):

```ini
[Node-appendChild.html]
  expected: OK

  [Appending a document]
    expected: FAIL

  [Appending a document fragment]
    expected: TIMEOUT
```

- The outer section names the test file; `expected` is the harness status
  (`OK`, `ERROR`, `TIMEOUT`, `CRASH`, `PRECONDITION_FAILED`).
- Each inner section names a subtest; `expected` is `PASS`, `FAIL`,
  `TIMEOUT`, `NOTRUN` or `PRECONDITION_FAILED`.
- Anything not listed is expected to be `OK`/`PASS`.
- A leading `# reason:` comment is encouraged for every non-pass entry.

Update workflow: run the directory, inspect `wpt/results/<product>/<dir>.json`,
then write or edit the `.ini` files by hand (or with
`python3 wpt/run.py update-expectations --product <p> --dir <dir>`, which
rewrites them from the last results file). Never mark a subtest as expected
`FAIL` to make CI green without a linked issue in the comment.
