"""Non-vacuity tests for wpt/run.py.

Every test here is a pure unit test over synthetic data: no driver binary, no
`./wpt serve`, no WPT checkout, no fixed port. That is not a shortcut, it is the
reason `judge`, `generate_candidate` and `promote` are separate pure functions --
the whole exit-code contract of the expectations round-trip can be exercised
without a run, so `compare` and `candidate` can be trusted on a machine that has
never checked out a test.

Run with:

    python3 -m unittest discover -s wpt/tests
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

RUN_PY = Path(__file__).resolve().parent.parent / "run.py"
_SPEC = importlib.util.spec_from_file_location("wpt_run", RUN_PY)
assert _SPEC is not None and _SPEC.loader is not None
wptrun = importlib.util.module_from_spec(_SPEC)
# `dataclasses` resolves annotations through `sys.modules[cls.__module__]`, so
# the module has to be registered before it executes.
sys.modules["wpt_run"] = wptrun
_SPEC.loader.exec_module(wptrun)

PRODUCT = "tentabrowser"
DIRECTORY = "dom/nodes"
ISSUE_42 = "https://github.com/tentaflow/tentabrowser/issues/42"
ISSUE_43 = "https://github.com/tentaflow/tentabrowser/issues/43"
ISSUE_44 = "https://github.com/tentaflow/tentabrowser/issues/44"

# Hand-written, not produced by `render_expectation`: a test that feeds the
# writer's output back into the parser would pass with both of them broken.
TWO_REVIEWED_FAILURES = f"""[Node-appendChild.html]

  [a]
    # reason: {ISSUE_42}
    expected: FAIL

  [b]
    # reason: {ISSUE_43}
    expected: FAIL
"""

UNREVIEWED_FAILURE = """[Node-appendChild.html]
  # bucket: missing-member Document.createProcessingInstruction
  # reason: unreviewed
  expected: ERROR

  [Appending a document]
    # bucket: missing-member Document.createProcessingInstruction
    # reason: unreviewed
    expected: FAIL
"""


def run_file(url, status="OK", message=None, subtests=(), duration_ms=1):
    """One synthetic driver result."""
    return {
        "test": url,
        "status": status,
        "message": message,
        "duration_ms": duration_ms,
        "subtests": [{"name": name, "status": sub_status, "message": sub_message} for name, sub_status, sub_message in subtests],
    }


def report(tests, **overrides):
    """A synthetic `wpt/results/<product>/<dir>.json`."""
    document = {
        "product": PRODUCT,
        "browser": {"name": PRODUCT, "version": "0.0.0", "backend": "v8"},
        "wpt_commit": "b89af32bc8f42d678f444eb0703bca015ddcf240",
        "directory": DIRECTORY,
        "started": "2026-09-21T00:00:00Z",
        "duration_s": 1.0,
        "summary": wptrun.summarize(tests),
        "driver": {"path": "/tmp/tenta-wpt-driver", "sha256": "a" * 64, "backend": "v8"},
        "plan_digest": "b" * 64,
        "truncated": False,
        "timeout_s": 10,
        "long_timeout_s": 60,
        "unexpected": [],
        "tests": tests,
    }
    document.update(overrides)
    return document


class BackendSelectionTestCase(unittest.TestCase):
    def test_products_keep_separate_results_and_expectations(self):
        with tempfile.TemporaryDirectory() as temporary:
            paths = wptrun.Paths(Path(temporary) / "expectations", Path(temporary) / "results")
            self.assertNotEqual(paths.ledger("tentabrowser"), paths.ledger("tentabrowser-qjs"))
            self.assertNotEqual(paths.results_file("tentabrowser", "dom/nodes"),
                                paths.results_file("tentabrowser-qjs", "dom/nodes"))
        parsed = wptrun.build_parser().parse_args(
            ["run", "--dir", "dom/nodes", "--product", "tentabrowser-qjs"])
        with mock.patch.object(wptrun, "resolve_driver", return_value=Path("/driver")):
            self.assertEqual(wptrun.make_driver(parsed).backend, "quickjs")

    def test_misfiled_and_unlabelled_reports_cannot_generate_or_promote(self):
        with tempfile.TemporaryDirectory() as temporary:
            paths = wptrun.Paths(Path(temporary) / "expectations", Path(temporary) / "results")
            product = "tentabrowser-qjs"
            candidate = paths.candidate_dir(product, DIRECTORY)
            candidate.mkdir(parents=True)
            sentinel = candidate / "keep.txt"
            sentinel.write_text("keep existing review")
            invalid = [
                (product, report([])),
                (product, report([], product=product)),
                (product, report([], product=product, browser={"backend": "quickjs"}, driver={})),
                (PRODUCT, report([], browser={}, driver={})),
            ]
            for selected, document in invalid:
                with self.subTest(document=document):
                    self.assertTrue(wptrun.comparability(selected, DIRECTORY, document,
                                                         paths=paths, baseline={}))
                    with self.assertRaises(wptrun.ToolError):
                        wptrun.generate_candidate(selected, DIRECTORY, document, paths=paths)
                    result = wptrun.promote(selected, DIRECTORY, document, paths=paths)
                    self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
                    self.assertFalse(paths.baseline(selected).exists())
                    self.assertEqual(sentinel.read_text(), "keep existing review")

    def test_plan_selects_backend_and_rejects_mismatched_driver_results(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            driver = wptrun.TentaBrowserDriver(Path("/driver"), "quickjs")
            def invoke(command, **kwargs):
                selected = json.loads(Path(command[1]).read_text())
                self.assertEqual(selected["backend"], "quickjs")
                Path(command[2]).write_text(json.dumps({
                    "browser": {"backend": returned_backend}, "tests": []}))
            with mock.patch.object(wptrun, "RESULTS", directory), \
                 mock.patch.object(wptrun.subprocess, "run", side_effect=invoke):
                returned_backend = "quickjs"
                self.assertEqual(driver.run({"tests": []})["tests"], [])
                for returned_backend in ("v8", None):
                    with self.subTest(returned_backend=returned_backend):
                        with self.assertRaises(wptrun.ToolError):
                            driver.run({"tests": []})


class RoundTripTestCase(unittest.TestCase):
    """A temporary expectations/results root, so nothing touches the real tree."""

    def setUp(self):
        holder = tempfile.TemporaryDirectory()
        self.addCleanup(holder.cleanup)
        root = Path(holder.name)
        self.paths = wptrun.Paths(expectations=root / "expectations", results=root / "results")
        self.root = root

    def write_expectation(self, url, text, product=PRODUCT):
        path = self.paths.expectation(product, url)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def judge(self, tests, product=PRODUCT):
        return wptrun.judge(tests, wptrun.load_expectation_tree(product, self.paths))

    def generate(self, tests, **overrides):
        document = report(tests, **overrides)
        return wptrun.generate_candidate(PRODUCT, DIRECTORY, document, paths=self.paths), document

    def review_candidate(self, reason=ISSUE_44, product=PRODUCT, directory=DIRECTORY):
        """Stand in for the human: replace every `unreviewed` sentinel with a reason."""
        root = self.paths.candidate_dir(product, directory)
        for path in sorted(root.rglob("*.ini")):
            path.write_text(path.read_text().replace(wptrun.UNREVIEWED, reason))

    def accept_buckets(self, tests, status="accepted", product=PRODUCT):
        """A ledger whose rows match this run exactly, so only the tested rule can fire."""
        rows = {
            key: wptrun.BucketRow(key=key, label="synthetic", issue=ISSUE_44, status=status, entries=count)
            for key, count in wptrun.bucket_counts(tests).items()
        }
        path = self.paths.ledger(product)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(wptrun.render_ledger(rows))
        return rows

    def bucket_key(self, tests):
        keys = list(wptrun.bucket_counts(tests))
        self.assertEqual(len(keys), 1, f"expected one bucket, got {keys}")
        return keys[0]


class JudgeUnreviewedTest(RoundTripTestCase):
    def test_judge_reports_an_unreviewed_entry_as_unexpected(self):
        """Generation alone must never turn a run green."""
        url = "/dom/nodes/Node-appendChild.html"
        self.write_expectation(url, UNREVIEWED_FAILURE)
        tests = [run_file(url, status="ERROR", message="boom", subtests=[("Appending a document", "FAIL", "boom")])]

        issues = self.judge(tests)

        self.assertEqual([issue.kind for issue in issues], ["unreviewed", "unreviewed"], issues)
        self.assertEqual([issue.entry for issue in issues], ["", "Appending a document"])
        for issue in issues:
            self.assertEqual(issue.test, url)
            self.assertIn("never reviewed", issue.detail)
        # The statuses match the expectation exactly: the review is the finding.
        self.assertNotIn("harness", [issue.kind for issue in issues])
        self.assertNotIn("subtest", [issue.kind for issue in issues])

    def test_judge_is_clean_when_every_non_pass_entry_has_a_reason(self):
        """A reviewed tree is the other half of the round-trip: no issues at all."""
        url = "/dom/nodes/Node-appendChild.html"
        self.write_expectation(url, TWO_REVIEWED_FAILURES)
        tests = [run_file(url, subtests=[("a", "FAIL", "boom"), ("b", "FAIL", "boom")])]

        self.assertEqual(self.judge(tests), [])

        # ... and a `PASS` entry never needs a reason.
        passing = [run_file(url, subtests=[("a", "PASS", None), ("b", "PASS", None)])]
        expectations = {url: wptrun.Expectation(harness=wptrun.Expected("OK"), subtests={})}
        self.assertEqual(wptrun.judge(passing, expectations), [])


class JudgeChangedStatusTest(RoundTripTestCase):
    def test_judge_flags_a_changed_status_in_both_directions(self):
        url = "/dom/nodes/Node-appendChild.html"
        self.write_expectation(url, TWO_REVIEWED_FAILURES)

        with self.subTest(direction="FAIL -> PASS"):
            tests = [run_file(url, subtests=[("a", "PASS", None), ("b", "PASS", None)])]
            issues = self.judge(tests)
            self.assertEqual([(issue.kind, issue.entry) for issue in issues], [("subtest", "a"), ("subtest", "b")])
            self.assertIn("PASS (expected FAIL)", issues[0].detail)

        with self.subTest(direction="PASS -> FAIL"):
            tests = [run_file(url, subtests=[("untracked", "FAIL", "boom")])]
            issues = self.judge(tests)
            self.assertEqual([(issue.kind, issue.entry) for issue in issues], [("subtest", "untracked")])
            self.assertIn("FAIL (expected PASS)", issues[0].detail)

        with self.subTest(direction="harness PASS -> TIMEOUT"):
            tests = [run_file(url, status="TIMEOUT", message="Test timed out", subtests=[("a", "NOTRUN", None)])]
            issues = self.judge(tests)
            harness = [issue for issue in issues if issue.kind == "harness"]
            self.assertEqual(len(harness), 1, issues)
            self.assertIn("harness TIMEOUT (expected OK)", harness[0].line())

        with self.subTest(direction="harness TIMEOUT -> OK"):
            other = "/dom/nodes/Timeouted.html"
            self.write_expectation(other, f"[Timeouted.html]\n  # reason: {ISSUE_42}\n  expected: TIMEOUT\n")
            tests = [run_file(url, subtests=[("a", "FAIL", "boom"), ("b", "FAIL", "boom")]), run_file(other)]
            issues = self.judge(tests)
            harness = [issue for issue in issues if issue.kind == "harness"]
            self.assertEqual([issue.test for issue in harness], [other])
            self.assertIn("harness OK (expected TIMEOUT)", harness[0].line())


class JudgeStaleExpectationTest(RoundTripTestCase):
    def test_judge_flags_a_stale_expectation_file(self):
        gone = "/dom/nodes/Gone.html"
        self.write_expectation(gone, f"[Gone.html]\n  # reason: {ISSUE_42}\n  expected: TIMEOUT\n")
        tests = [run_file("/dom/nodes/Present.html")]

        issues = self.judge(tests)

        self.assertEqual([(issue.kind, issue.test) for issue in issues], [("stale", gone)])
        self.assertIn("stale expectation", issues[0].detail)


class RegenerationMergeTest(RoundTripTestCase):
    def test_regeneration_preserves_reviewed_reasons(self):
        """A regeneration diff is only what changed, and generation never writes live."""
        url = "/dom/nodes/Node-appendChild.html"
        live = self.write_expectation(url, TWO_REVIEWED_FAILURES)
        tests = [
            run_file(
                url,
                subtests=[
                    ("a", "FAIL", "Cannot read properties of undefined (reading 'createProcessingInstruction')"),
                    ("b", "FAIL", "boom"),
                ],
            )
        ]

        first, document = self.generate(tests)
        candidate = first.root / wptrun.candidate_relative(DIRECTORY, url)
        text_one = candidate.read_text()
        self.assertTrue(candidate.is_file())
        self.assertTrue(str(candidate).startswith(str(self.paths.results)), "candidates live under wpt/results/")
        self.assertIn(f"# reason: {ISSUE_42}", text_one)
        self.assertIn(f"# reason: {ISSUE_43}", text_one)
        self.assertEqual(text_one.count(wptrun.UNREVIEWED), 0)
        # Decision 1: generation writes a candidate and never the live tree.
        self.assertEqual(live.read_text(), TWO_REVIEWED_FAILURES)

        # Blessing it, then regenerating over the same results, is a fixed point.
        wptrun.apply_promotion(PRODUCT, DIRECTORY, document, paths=self.paths, blessed_by="test")
        blessed = live.read_text()
        self.assertNotEqual(blessed, TWO_REVIEWED_FAILURES)
        second, _ = self.generate(tests)
        self.assertEqual((second.root / wptrun.candidate_relative(DIRECTORY, url)).read_text(), blessed)

        # One mutated status leaves exactly that entry unreviewed.
        mutated = [
            run_file(
                url,
                subtests=[
                    ("a", "TIMEOUT", None),
                    ("b", "FAIL", "boom"),
                ],
            )
        ]
        third, _ = self.generate(mutated)
        text_three = (third.root / wptrun.candidate_relative(DIRECTORY, url)).read_text()
        self.assertEqual(text_three.count(wptrun.UNREVIEWED), 1)
        self.assertIn("expected: TIMEOUT", text_three)
        self.assertNotIn(f"# reason: {ISSUE_42}", text_three)
        self.assertIn(f"# reason: {ISSUE_43}", text_three)

        # An entry that no longer fails is dropped from the candidate.
        shrinking = [run_file(url, subtests=[("a", "FAIL", "boom")])]
        fourth, _ = self.generate(shrinking)
        text_four = (fourth.root / wptrun.candidate_relative(DIRECTORY, url)).read_text()
        self.assertEqual(fourth.dropped, 1)
        self.assertNotIn("[b]", text_four)


class PromotionRefusalTest(RoundTripTestCase):
    def test_promotion_refuses_never_blessable_results(self):
        crash = run_file("/dom/nodes/Crashed.html", status="CRASH", message="page crashed")
        deadline = run_file("/dom/nodes/Deadline.html", status="NOTRUN", message="run deadline reached")
        silent = run_file(
            "/dom/nodes/Silent.svg",
            status="TIMEOUT",
            message="the completion hook did not arrive within 10000 ms",
        )
        tests = [crash, deadline, silent]
        _, document = self.generate(tests)
        self.review_candidate()
        self.accept_buckets(tests, status="must-fix")

        result = wptrun.promote(
            PRODUCT,
            DIRECTORY,
            document,
            paths=self.paths,
            control={silent["test"]: 12},
        )

        self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
        refusals = "\n".join(result.refusals)
        self.assertIn(crash["test"], refusals)
        self.assertIn("CRASH is never blessable", refusals)
        self.assertIn(deadline["test"], refusals)
        self.assertIn("truncation artefact", refusals)
        self.assertIn(silent["test"], refusals)
        self.assertIn("chromium control reports 12 subtests", refusals)
        # Nothing crossed the seam.
        self.assertFalse(self.paths.expectation(PRODUCT, crash["test"]).exists())
        self.assertFalse(self.paths.baseline(PRODUCT).exists())

        # ... and there is no override to reach for.
        subparsers = [action for action in wptrun.build_parser()._actions if isinstance(action, argparse._SubParsersAction)]
        self.assertEqual(len(subparsers), 1)
        options = {
            option for action in subparsers[0].choices["promote"]._actions for option in action.option_strings
        }
        self.assertEqual(options, {"-h", "--help", "--dir", "--product", "--by"}, options)

    def test_promotion_refuses_a_truncated_or_non_comparable_run(self):
        tests = [run_file("/dom/nodes/A.html", subtests=[("x", "FAIL", "(reading 'observable')")])]

        with self.subTest(reason="truncated"):
            _, document = self.generate(tests, truncated=True)
            self.review_candidate()
            self.accept_buckets(tests)
            result = wptrun.promote(PRODUCT, DIRECTORY, document, paths=self.paths)
            self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
            self.assertTrue(any("truncated" in refusal for refusal in result.refusals), result.refusals)

        with self.subTest(reason="baseline stamp"):
            baseline = self.paths.baseline(PRODUCT)
            baseline.parent.mkdir(parents=True, exist_ok=True)
            baseline.write_text(
                wptrun.render_baseline(
                    {
                        DIRECTORY: {
                            "wpt_commit": "b89af32bc8f42d678f444eb0703bca015ddcf240",
                            "driver_sha256": "f" * 64,
                            "plan_digest": "b" * 64,
                        }
                    }
                )
            )
            before = baseline.read_text()
            _, document = self.generate(tests)
            self.review_candidate()
            self.accept_buckets(tests)
            result = wptrun.promote(PRODUCT, DIRECTORY, document, paths=self.paths)
            self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
            named = [refusal for refusal in result.refusals if "driver_sha256 differs" in refusal]
            self.assertEqual(len(named), 1, result.refusals)
            # A refused promotion writes nothing, not even the stamp it failed on.
            self.assertEqual(baseline.read_text(), before)

    def test_promotion_refuses_a_bucket_that_grew(self):
        tests = [
            run_file(f"/dom/nodes/F{index}.html", subtests=[("x", "FAIL", "Observable is not defined")])
            for index in range(246)
        ]
        key = self.bucket_key(tests)
        self.assertEqual(key, "missing-global Observable")

        rows = self.accept_buckets(tests)
        self.assertEqual(rows[key].entries, 246)

        with self.subTest(direction="grew"):
            rows[key].entries = 244
            self.write_ledger(rows)
            _, document = self.generate(tests)
            self.review_candidate()
            result = wptrun.promote(PRODUCT, DIRECTORY, document, paths=self.paths)
            self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
            self.assertTrue(
                any(
                    f"bucket {key!r} grew: this run has 246 entries, the ledger blessed 244" in refusal
                    for refusal in result.refusals
                ),
                result.refusals,
            )
            self.assertEqual(list(self.paths.expectation_root(PRODUCT).rglob("*.ini")), [])

        with self.subTest(direction="shrank"):
            rows[key].entries = 248
            self.write_ledger(rows)
            _, document = self.generate(tests)
            self.review_candidate()
            result = wptrun.promote(PRODUCT, DIRECTORY, document, paths=self.paths)
            self.assertEqual(result.exit_code, wptrun.EXIT_REFUSED)
            self.assertTrue(any("shrank" in refusal for refusal in result.refusals), result.refusals)

        with self.subTest(direction="matched"):
            rows[key].entries = 246
            self.write_ledger(rows)
            _, document = self.generate(tests)
            self.review_candidate()
            result = wptrun.promote(PRODUCT, DIRECTORY, document, paths=self.paths)
            self.assertEqual(result.exit_code, wptrun.EXIT_OK, result.refusals)
            self.assertEqual(result.files_written, 246)
            self.assertEqual(len(list(self.paths.expectation_root(PRODUCT).rglob("*.ini"))), 246)

    def write_ledger(self, rows, product=PRODUCT):
        path = self.paths.ledger(product)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(wptrun.render_ledger(rows))


if __name__ == "__main__":
    unittest.main()
