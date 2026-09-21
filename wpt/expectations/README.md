# WPT expectations

Expectation files record the results a product is known to produce so that
`wpt/run.py` flags regressions (unexpected failures) and improvements
(unexpected passes). They are a **signed statement about the engine**, not
output: `wpt/run.py`'s exit status has to mean "the engine changed", never "the
instrument changed".

Layout: `wpt/expectations/<product>/<wpt path>.ini`, one file per test file,
mirroring the WPT tree. Example: the expectations for
`/dom/nodes/Node-appendChild.html` on `tentabrowser` live in
`wpt/expectations/tentabrowser/dom/nodes/Node-appendChild.html.ini`.

## Format

A subset of wptrunner's metadata syntax, plus two review comments:

```ini
[Node-appendChild.html]
  # bucket: harness-error:20 duplicate test names
  # reason: https://github.com/tentaflow/tentabrowser/issues/123
  expected: ERROR

  [Appending a document]
    # bucket: missing-member Document.createProcessingInstruction
    # reason: https://github.com/tentaflow/tentabrowser/issues/124
    expected: FAIL
```

- The outer section names the test file; `expected` is the harness status
  (`OK`, `ERROR`, `TIMEOUT`, `CRASH`, `PRECONDITION_FAILED`, `NOTRUN`).
- Each inner section names a subtest; `expected` is `PASS`, `FAIL`, `TIMEOUT`,
  `NOTRUN` or `PRECONDITION_FAILED`.
- Anything not listed is expected to be `OK`/`PASS`.
- `# bucket:` and `# reason:` attach to the section they sit in. **Every
  non-pass entry must carry both.** The reason names the defect; an issue link
  is the usual form.
- `wpt/results/<product>/candidate/**` is generated output. **Never hand-write
  an `.ini`** outside the review loop below; `judge` reads the whole tree and a
  file whose test no longer exists is itself an unexpected result (a *stale
  expectation*).

### `unreviewed`

The generator marks an entry whose status is **new or changed** with
`# reason: unreviewed`. That word is a sentinel, not a review: the judge treats
it exactly like a status mismatch. Generating an expectation tree therefore
never turns a run green on its own — the run stays red until a human replaces
the sentinel with a reason. An entry whose status did **not** change re-emits
verbatim with its reason intact, so after the first blessing a regeneration
diff is only what actually changed.

Do not mass-replace the sentinel with a script. The review unit is the bucket
ledger (`BUCKETS.toml`), never `grep -c unreviewed`.

## The bucket ledger

`wpt/expectations/<product>/BUCKETS.toml` is the review unit:

```toml
[[bucket]]
key = "missing-global Observable"
label = "the whole dom/observable directory is one absent interface"
issue = "https://github.com/tentaflow/tentabrowser/issues/130"
status = "accepted"     # accepted | must-fix | forbidden
entries = 244           # the count blessed for the directory set reviewed here
```

The bucket key is derived mechanically from `(status, message)` — a message that
gains a character upstream must not create a new bucket. The rules live in
`harness_bucket`/`subtest_bucket` in `wpt/run.py`; the shapes are
`missing-member X`, `missing-function X`, `missing-global X`, `illegal-constructor`,
`unsupported-feature <token>`, `assert-mismatch:<test>`, `notrun-after-<parent>`,
`timeout-harness-report`, `timeout-no-subtests`, `harness-error:<60 chars>` and a
`per-file:<test>` fallback for the rest.

**The bucket is review metadata, not comparison semantics.** A wrong derivation
can never make a run green or red by itself. What it can do is make a defect
reviewable.

A tree cannot be reviewed entry by entry and the docs should not pretend
otherwise: `dom` alone is ~265 files and ~13 000 lines, and the nine P2
directories project to 800-1500 files and 30 000-60 000 entries. The ledger is
a few dozen rows for `dom` and a few hundred repo-wide. **Read the ledger and
the delta table in the review sheet; diff the tree.**

`entries` is the count the ledger blessed **for the directory set it was
reviewed over**. A run over a different set will legitimately differ, so
re-bless the ledger when the set of blessed directories grows.

## Never blessable

These shapes must not become an expectation, whatever the ledger says. `promote`
refuses them and there is **no override flag**:

- a `CRASH` — a crashed page reports nothing about the engine;
- a **harness-level** `NOTRUN` whose message is `run deadline reached` — a
  truncation artefact, and blessing it makes the next untruncated run report an
  unexpected pass forever;
- a **driver-attributed** `TIMEOUT` (`the completion hook did not arrive …`)
  with zero subtests on a file whose Chromium control reports subtests — pre-S2
  that shape is a driver defect, post-S2 a survivor is a *finding*;
- a harness `ERROR` carrying `N duplicate test names` unless the reason names
  the interface-mapping defect it hides **and** its ledger row is `must-fix`.

A `PRECONDITION_FAILED` (bucket `unsupported-feature <token>`) and a subtest
`NOTRUN` under a parent that names the throw may be blessed. A harness `NOTRUN`
never may.

## Workflow

```bash
# 1. measure (writes wpt/results/<product>/<dir>.json and judges it; exit 0/1/3)
python3 wpt/run.py run --product tentabrowser --dir dom --jobs 4

# 2. generate a CANDIDATE under wpt/results/<product>/candidate/ -- the live
#    tree is untouched, and this is not a promotion
python3 wpt/run.py candidate --product tentabrowser --dir dom

# 3. read the review sheet: the bucket table, the delta, what is never blessable,
#    what still needs a reason
less wpt/results/tentabrowser/candidate/dom.review.txt

# 4. sign it off:
#    - merge the proposed ledger into wpt/expectations/<product>/BUCKETS.toml
#      (the proposal is at .../candidate/dom.BUCKETS.proposed.toml), giving each
#      row a label, an issue link and a deliberate `status`
#    - replace `unreviewed` with a reason in wpt/results/.../candidate/dom/**/*.ini,
#      including every entry printed under "files with entries that still need a
#      reason" in the review sheet

# 5. copy the reviewed candidate over the live tree and stamp BASELINE.toml.
#    The reasons land in tracked files here, so `git diff wpt/expectations/` is
#    the review of record
python3 wpt/run.py promote --product tentabrowser --dir dom --by "your name"

# 6. prove it: the same run must now exit 0
python3 wpt/run.py compare --product tentabrowser --dir dom     # no server needed
# or re-run:
```

`update-expectations` still parses, but it is a **deprecated alias for
`candidate`**: it writes a candidate and never the live tree.

`compare` re-judges a stored results file. It needs no checkout, no server and
no driver, which is what makes the judge testable (`wpt/tests/test_run.py`).

## Exit codes

| code | meaning |
|------|---------|
| 0 | comparable run, every result matches a **reviewed** expectation |
| 1 | comparable run, results differ — a regression, an improvement, or an entry with no review |
| 2 | tooling or usage error (no driver built, bad ledger `status`, a port in use). Never a statement about the engine |
| 3 | **not comparable**: a truncated run, a baseline-stamp mismatch, or a configured directory that yielded zero tests |
| 4 | `promote` refused; nothing was written |

Exit 3 names the stamp field that moved (`wpt_commit`, `driver_sha256`,
`plan_digest`), because "not comparable" must not become the new "ignore the
red". `wpt/expectations/<product>/BASELINE.toml` records those fields per
directory at blessing time.
