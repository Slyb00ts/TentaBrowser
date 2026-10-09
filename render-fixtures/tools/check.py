#!/usr/bin/env python3
"""Check a set of engine captures against the render fixture corpus.

For every fixture directory `render-fixtures/<name>/` this compares
`<name>/<engine>.png` (default engine: `tentabrowser`) with the committed
Chromium reference `<name>/chromium.png` and evaluates two kinds of checks:

- `color-checks.json`: solid-colour component geometry (imported from the
  reference corpus). Font anti-aliasing never decides pass/fail because only
  exact-colour connected components are measured. These are the behaviour
  assertions.
- pixel-distance metrics between the two captures (rgb MAE, pixels over
  thresholds, content/edge bounding boxes, projections, bidirectional edge
  distance). These are regression tripwires and diagnostics, never a
  standalone verdict (docs/spec/15-testing-and-benchmarks.md §1).

`checks.json` (element and text rects) is consumed by the Rust structural
test in tenta-render once P3 lands; this script does not need it.

Usage: check.py [--engine NAME] [--fixture NAME ...] [--report PATH]
Requires numpy, scipy and pillow (see docs/dev/render-fixtures.md).

Derived from `render-repros/check.py` of the reference implementation
(Apache-2.0, see ../ATTRIBUTION.md); the component and metric functions are
kept intact so the numbers stay comparable with the reference's reports.
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import (
    distance_transform_edt,
    find_objects,
    gaussian_filter,
    gaussian_filter1d,
    label,
    sobel,
)

CORPUS = Path(__file__).resolve().parent.parent


def rgb(value):
    value = value.removeprefix("#")
    return tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))


def components(array, color):
    mask = np.all(array == color, axis=2)
    labels, _ = label(mask)
    found = []
    for slices in find_objects(labels):
        if slices is None:
            continue
        height = slices[0].stop - slices[0].start
        width = slices[1].stop - slices[1].start
        area = int(mask[slices].sum())
        if area >= 20:
            found.append(
                {
                    "x": slices[1].start,
                    "y": slices[0].start,
                    "width": width,
                    "height": height,
                    "area": area,
                }
            )
    return found


def bbox(mask):
    ys, xs = np.nonzero(mask)
    if not len(xs):
        return None
    return [int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1]


def structural_edges(array):
    """Return major luminance edges while suppressing texture and antialiasing."""
    rgb = array.astype(np.float32)
    luminance = rgb[:, :, 0] * 0.2126 + rgb[:, :, 1] * 0.7152 + rgb[:, :, 2] * 0.0722
    smoothed = gaussian_filter(luminance, sigma=1.0)
    magnitude = np.hypot(sobel(smoothed, axis=0), sobel(smoothed, axis=1))
    return magnitude > 48.0


def bidirectional_edge_distance(ours_edges, chromium_edges):
    """Symmetric nearest-edge distances in native CSS-pixel space.

    Row and column projections discard the other coordinate, so two layouts can
    have identical projections while their boxes occupy different quadrants. A
    distance transform retains both coordinates and each engine contributes
    equally instead of letting the denser edge map dominate.
    """
    ours_count = int(ours_edges.sum())
    chromium_count = int(chromium_edges.sum())
    if ours_count == 0 and chromium_count == 0:
        return {
            "edge_bidirectional_mean_distance_px": 0.0,
            "edge_bidirectional_p95_distance_px": 0.0,
        }
    if ours_count == 0 or chromium_count == 0:
        return {
            "edge_bidirectional_mean_distance_px": None,
            "edge_bidirectional_p95_distance_px": None,
        }
    distance_to_chromium = distance_transform_edt(~chromium_edges)[ours_edges]
    distance_to_ours = distance_transform_edt(~ours_edges)[chromium_edges]
    mean_distance = (distance_to_chromium.mean() + distance_to_ours.mean()) / 2.0
    p95_distance = (np.percentile(distance_to_chromium, 95) + np.percentile(distance_to_ours, 95)) / 2.0
    return {
        "edge_bidirectional_mean_distance_px": round(float(mean_distance), 6),
        "edge_bidirectional_p95_distance_px": round(float(p95_distance), 6),
    }


def luminance(array):
    return (
        array[:, :, 0].astype(np.float32) * 0.2126
        + array[:, :, 1].astype(np.float32) * 0.7152
        + array[:, :, 2].astype(np.float32) * 0.0722
    )


def pair_metrics(ours, chromium):
    if ours.shape != chromium.shape:
        return {"size_mismatch": [list(ours.shape), list(chromium.shape)]}
    delta = np.abs(ours.astype(np.int16) - chromium.astype(np.int16))
    max_channel = delta.max(axis=2)
    ours_ink = np.any(ours < 245, axis=2)
    chromium_ink = np.any(chromium < 245, axis=2)
    ours_bbox = bbox(ours_ink)
    chromium_bbox = bbox(chromium_ink)
    bbox_delta = None
    if ours_bbox and chromium_bbox:
        bbox_delta = max(abs(a - b) for a, b in zip(ours_bbox, chromium_bbox))
    height, width = ours_ink.shape
    row_projection = float(np.abs(ours_ink.sum(axis=1) - chromium_ink.sum(axis=1)).mean() / width)
    col_projection = float(np.abs(ours_ink.sum(axis=0) - chromium_ink.sum(axis=0)).mean() / height)
    # A fixed `<245` "ink" threshold treats a uniform light page background as
    # content; blurred major edges retain box boundaries and glyph rows while
    # ignoring background colour, low-amplitude texture and font antialiasing.
    ours_edges = structural_edges(ours)
    chromium_edges = structural_edges(chromium)
    ours_edge_bbox = bbox(ours_edges)
    chromium_edge_bbox = bbox(chromium_edges)
    edge_bbox_delta = None
    if ours_edge_bbox and chromium_edge_bbox:
        edge_bbox_delta = max(abs(a - b) for a, b in zip(ours_edge_bbox, chromium_edge_bbox))
    ours_edge_rows = gaussian_filter1d(ours_edges.sum(axis=1).astype(float), sigma=2.0)
    chromium_edge_rows = gaussian_filter1d(chromium_edges.sum(axis=1).astype(float), sigma=2.0)
    ours_edge_cols = gaussian_filter1d(ours_edges.sum(axis=0).astype(float), sigma=2.0)
    chromium_edge_cols = gaussian_filter1d(chromium_edges.sum(axis=0).astype(float), sigma=2.0)
    metrics = {
        "rgb_mae": round(float(delta.mean() / 255.0), 6),
        "pixels_gt_10": round(float((max_channel > 10).mean()), 6),
        "pixels_gt_50": round(float((max_channel > 50).mean()), 6),
        "ours_content_bbox": ours_bbox,
        "chromium_content_bbox": chromium_bbox,
        "content_bbox_max_delta": bbox_delta,
        "row_projection_delta": round(row_projection, 6),
        "column_projection_delta": round(col_projection, 6),
        "ours_edge_bbox": ours_edge_bbox,
        "chromium_edge_bbox": chromium_edge_bbox,
        "edge_bbox_max_delta": edge_bbox_delta,
        "edge_row_projection_delta": round(float(np.abs(ours_edge_rows - chromium_edge_rows).mean() / width), 6),
        "edge_column_projection_delta": round(float(np.abs(ours_edge_cols - chromium_edge_cols).mean() / height), 6),
        # A matching solid canvas is not evidence of fidelity; keep per-engine
        # signal so blank captures can be excluded without discarding deltas.
        "ours_luminance_stddev": round(float(luminance(ours).std()), 6),
        "chromium_luminance_stddev": round(float(luminance(chromium).std()), 6),
        "ours_structural_edge_pixels": int(ours_edges.sum()),
        "chromium_structural_edge_pixels": int(chromium_edges.sum()),
    }
    metrics.update(bidirectional_edge_distance(ours_edges, chromium_edges))
    return metrics


def evaluate_color_checks(array, checks):
    results = []
    for check in checks:
        expected_count = check.get("count", 1)
        matches = []
        for component in components(array, rgb(check["color"])):
            close = all(
                key not in check or abs(component[key] - check[key]) <= 1 for key in ("x", "y", "width", "height")
            )
            if close:
                matches.append(component)
        results.append(
            {
                "name": check["name"],
                "passed": len(matches) >= expected_count,
                "expected_count": expected_count,
                "matches": matches,
            }
        )
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--engine", default="tentabrowser", help="capture file name without .png (default: tentabrowser)")
    parser.add_argument("--fixture", action="append", default=[], help="fixture name (repeatable; default: all)")
    parser.add_argument("--report", type=Path, help="write the full JSON report here")
    args = parser.parse_args()

    names = args.fixture or sorted(p.name for p in CORPUS.iterdir() if (p / "index.html").is_file())
    failures = []
    report = {"engine": args.engine, "fixtures": {}}
    for name in names:
        directory = CORPUS / name
        ours_path = directory / f"{args.engine}.png"
        chromium_path = directory / "chromium.png"
        if not ours_path.is_file():
            failures.append(f"{name}: missing {ours_path.name}")
            continue
        if not chromium_path.is_file():
            failures.append(f"{name}: missing chromium.png")
            continue
        ours = np.asarray(Image.open(ours_path).convert("RGB"))
        chromium = np.asarray(Image.open(chromium_path).convert("RGB"))
        color_checks_path = directory / "color-checks.json"
        checks = json.loads(color_checks_path.read_text()) if color_checks_path.is_file() else []
        fixture = {
            "metrics": pair_metrics(ours, chromium),
            "behavior": {args.engine: evaluate_color_checks(ours, checks), "chromium": evaluate_color_checks(chromium, checks)},
        }
        for engine, results in fixture["behavior"].items():
            for result in results:
                if not result["passed"]:
                    failures.append(
                        f"{name} {engine}: {result['name']} expected {result['expected_count']}, "
                        f"found {len(result['matches'])}"
                    )
        report["fixtures"][name] = fixture
        metrics = fixture["metrics"]
        # Metrics are None when a capture has no edges or no ink.
        def cell(key, default="-"):
            return str(metrics.get(key, default))

        print(
            f"{name:36} rgb_mae={cell('rgb_mae', 'size-mismatch'):>8} "
            f"p>50={cell('pixels_gt_50'):>8} "
            f"edge_bbox={cell('edge_bbox_max_delta'):>4} "
            f"edge_2d={cell('edge_bidirectional_mean_distance_px'):>8}"
        )
    report["failures"] = failures
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    if failures:
        for failure in failures:
            print(f"FAILED: {failure}", file=sys.stderr)
        raise SystemExit(1)
    print(f"behaviour checks passed for {len(report['fixtures'])} fixtures")


if __name__ == "__main__":
    main()
