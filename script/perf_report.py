#!/usr/bin/env python3
"""Render the Haskell vs Rust performance report from perf_compare.py JSON.

usage: perf_report.py RESULTS_JSON OUT_DIR

Writes OUT_DIR/README.md and the SVG charts it embeds. Run from the repository root.
"""

import json
import math
import sys
from html import escape
from pathlib import Path

results = json.loads(Path(sys.argv[1]).read_text())
out_dir = Path(sys.argv[2])
floor = results["rss_floor_mb"]
heavy, report, small, translation = results["groups"]

STYLE = """
<style>
  svg { font-family: system-ui, -apple-system, "Segoe UI", sans-serif; font-variant-numeric: tabular-nums; }
  .surface { fill: #fcfcfb; }
  .ink { fill: #0b0b0b; }
  .ink2 { fill: #52514e; }
  .muted { fill: #898781; }
  .grid { stroke: #e1e0d9; }
  .axis { stroke: #c3c2b7; }
  .ref { stroke: #52514e; }
  .hs { fill: #2a78d6; }
  .rs { fill: #eb6834; }
  @media (prefers-color-scheme: dark) {
    .surface { fill: #1a1a19; }
    .ink { fill: #f0efec; }
    .ink2 { fill: #c3c2b7; }
    .grid { stroke: #2c2c2a; }
    .axis { stroke: #383835; }
    .ref { stroke: #c3c2b7; }
    .hs { fill: #3987e5; }
    .rs { fill: #d95926; }
  }
</style>
"""

WIDTH, LEFT, RIGHT, TOP = 760, 220, 64, 56
BAR, GAP, ROW_GAP = 12, 2, 16


def nice_ticks(limit, count=5):
    raw = limit / count
    step = 10 ** math.floor(math.log10(raw))
    step *= next(m for m in (1, 2, 2.5, 5, 10) if step * m >= raw)
    return [i * step for i in range(int(limit // step) + 2)]


def bar(x, y, w, h, cls, tip):
    r = min(4, w, h / 2)
    path = f"M{x},{y} H{x + w - r} A{r},{r} 0 0 1 {x + w},{y + r} V{y + h - r} A{r},{r} 0 0 1 {x + w - r},{y + h} H{x} Z"
    return f'<path class="{cls}" d="{path}"><title>{escape(tip)}</title></path>'


def chart(path, title, rows, series, fmt, unit, ref=None):
    """rows: [(label, [value per series])], series: [(css class, legend label)]."""
    per_row = len(series) * BAR + (len(series) - 1) * GAP
    height = TOP + len(rows) * (per_row + ROW_GAP) + 34
    plot = WIDTH - LEFT - RIGHT
    ticks = nice_ticks(max(v for _, values in rows for v in values))
    scale = plot / ticks[-1]
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" role="img" aria-label="{escape(title)}">',
        STYLE,
    ]
    out.append(f'<rect class="surface" width="{WIDTH}" height="{height}" rx="8"/>')
    out.append(
        f'<text class="ink" x="16" y="26" font-size="15" font-weight="600">{escape(title)}</text>'
    )
    lx = LEFT
    for cls, label in series if len(series) > 1 else []:
        out.append(
            f'<rect class="{cls}" x="{lx}" y="36" width="10" height="10" rx="2"/>'
        )
        out.append(
            f'<text class="ink2" x="{lx + 15}" y="45" font-size="12">{escape(label)}</text>'
        )
        lx += 30 + 7 * len(label)
    bottom = height - 34
    for t in ticks:
        x = LEFT + t * scale
        out.append(
            f'<line class="grid" x1="{x}" y1="{TOP - 4}" x2="{x}" y2="{bottom}" stroke-width="1"/>'
        )
        out.append(
            f'<text class="muted" x="{x}" y="{bottom + 16}" font-size="11" text-anchor="middle">{fmt(t)}</text>'
        )
    out.append(
        f'<text class="muted" x="{WIDTH - RIGHT}" y="{bottom + 30}" font-size="11" text-anchor="end">{escape(unit)}</text>'
    )
    y = TOP
    for label, values in rows:
        out.append(
            f'<text class="ink" x="{LEFT - 10}" y="{y + per_row / 2 + 4}" font-size="12" text-anchor="end">{escape(label)}</text>'
        )
        for (cls, name), v in zip(series, values):
            out.append(
                bar(
                    LEFT,
                    y,
                    max(v * scale, 1),
                    BAR,
                    cls,
                    f"{label}, {name}: {fmt(v)} {unit}",
                )
            )
            out.append(
                f'<text class="ink2" x="{LEFT + v * scale + 6}" y="{y + BAR - 2}" font-size="11">{fmt(v)}</text>'
            )
            y += BAR + GAP
        y += ROW_GAP - GAP
    out.append(
        f'<line class="axis" x1="{LEFT}" y1="{TOP - 4}" x2="{LEFT}" y2="{bottom}" stroke-width="1"/>'
    )
    if ref:
        x = LEFT + ref[0] * scale
        out.append(
            f'<line class="ref" x1="{x}" y1="{TOP - 4}" x2="{x}" y2="{bottom}" stroke-width="1.5" stroke-dasharray="4 3"/>'
        )
        out.append(
            f'<text class="ink2" x="{x + 4}" y="{TOP - 8}" font-size="11">{escape(ref[1])}</text>'
        )
    out.append("</svg>")
    (out_dir / path).write_text("\n".join(out) + "\n")


def total(group, key):
    return sum(c[key] for c in group["cases"])


def speedup(group):
    return total(group, "haskell_ms") / total(group, "rust_ms")


def mb(v):
    return f"<= {floor:.0f}" if v <= floor + 0.5 else f"{v:.1f}"


big = heavy["cases"] + report["cases"]
SERIES = [("hs", "Haskell"), ("rs", "Rust")]
chart(
    "time.svg",
    "Wall time on heavy workloads (lower is better)",
    [(c["name"], [c["haskell_ms"] / 1000, c["rust_ms"] / 1000]) for c in big],
    SERIES,
    lambda v: f"{v:.1f}" if v < 10 else f"{v:.0f}",
    "seconds, median",
)
speed_rows = [(c["name"], [c["haskell_ms"] / c["rust_ms"]]) for c in big]
speed_rows += [
    (f"{c['name']} (-S)", [c["haskell_ms"] / c["rust_ms"]])
    for c in translation["cases"]
]
speed_rows.append((f"{len(small['cases'])} small examples, total", [speedup(small)]))
chart(
    "speedup.svg",
    "Rust speedup over Haskell (higher is better)",
    speed_rows,
    [("rs", "Rust vs Haskell")],
    lambda v: f"{v:.1f}x",
    "times faster",
    ref=(1, "1x = same speed"),
)
chart(
    "memory.svg",
    "Peak memory on heavy workloads (lower is better)",
    [(c["name"], [c["haskell_rss_mb"], c["rust_rss_mb"]]) for c in big],
    SERIES,
    lambda v: f"{v:.0f}",
    "MB, max RSS",
)


def table(group):
    lines = [
        "| case | Haskell, ms | Rust, ms | speedup | Haskell RSS, MB | Rust RSS, MB | exit code |",
        "| --- | ---: | ---: | ---: | ---: | ---: | :---: |",
    ]
    for c in group["cases"]:
        same = c["haskell_exit"] == c["rust_exit"]
        codes = (
            "/".join(map(str, c["rust_exit"]))
            if same
            else f"differs: {c['haskell_exit']} vs {c['rust_exit']}"
        )
        lines.append(
            f"| {c['name']} | {c['haskell_ms']:.1f} | {c['rust_ms']:.1f} | {c['haskell_ms'] / c['rust_ms']:.2f}x"
            f" | {mb(c['haskell_rss_mb'])} | {mb(c['rust_rss_mb'])} | {codes} |"
        )
    lines.append(
        f"| **total** | **{total(group, 'haskell_ms'):.0f}** | **{total(group, 'rust_ms'):.0f}** | **{speedup(group):.2f}x** | | | |"
    )
    return "\n".join(lines)


def workloads():
    lines = ["| program | ISA | what it does |", "| --- | --- | --- |"]
    for conf in sorted(Path("test/performance").glob("*.yaml")):
        src = "program.s" if conf.stem == "conf" else f"{conf.stem}.s"
        isa = "f32a" if conf.stem == "conf" else "risc-iv-32"
        desc = conf.read_text().splitlines()[0].removeprefix("name: ")
        lines.append(
            f"| [`{src}`]({src}) + [`{conf.name}`]({conf.name}) | {isa} | {desc} |"
        )
    return "\n".join(lines)


heavy_mem = sum(c["haskell_rss_mb"] for c in big) / sum(c["rust_rss_mb"] for c in big)
small_ms = total(small, "rust_ms") / len(small["cases"])
small_hs_ms = total(small, "haskell_ms") / len(small["cases"])
host = results["host"]

(out_dir / "README.md").write_text(f"""# Performance: Haskell vs Rust

How the Rust rewrite of `wrench` compares to the original Haskell implementation on the same programs and configs.

| | |
| --- | --- |
| **{speedup(heavy):.1f}x faster** | heavy simulations, total wall time ({total(heavy, "haskell_ms") / 1000:.0f} s -> {total(heavy, "rust_ms") / 1000:.0f} s) |
| **{speedup(report):.1f}x faster** | a huge step-by-step report (`slice: all`) |
| **{heavy_mem:.1f}x less memory** | peak RSS on the heavy workloads |
| **{speedup(small):.1f}x faster** | small examples, mostly process startup (~{small_hs_ms:.0f} ms -> ~{small_ms:.0f} ms per run) |
| **{speedup(translation):.1f}x faster** | translation only (`-S`) |

![Wall time on heavy workloads](time.svg)

![Rust speedup over Haskell](speedup.svg)

![Peak memory on heavy workloads](memory.svg)

## Setup

- Haskell: `{results["haskell_version"]}`, the last Haskell commit (`10eb83a`), built with `stack build --ghc-options -O2` in the `ryukzak/wrench-builder` image.
- Rust: `{results["rust_version"]}`, built with `cargo build --release`.
- Host: {host["cpu"]}, {host["cpus"]} CPUs, {host["os"]}. A shared machine with other services running, so expect a few percent of noise.
- Each case runs once as warmup, then the median wall time of {heavy["runs"]} runs (heavy) or {small["runs"]} runs (small and `-S`) is taken. Memory is the max RSS over the runs.
- RSS shown as `<= {floor:.0f}` means the process stayed under the memory the benchmark driver itself had when it started the process, so the real value is lower.

## Workloads

Heavy programs live in this directory. Each reads `n` from `0x80`, writes the result to `0x84` and asserts it, so both versions are checked to compute the same thing.

{workloads()}

The small examples are every `example/<isa>/*.yaml` with its program, and `-S` runs only translation.

## Results

### {heavy["title"]}

{table(heavy)}

### {report["title"]}

{table(report)}

### {small["title"]}

{table(small)}

### {translation["title"]}

{table(translation)}

## Reproduce

```shell
cargo build --release --bin wrench
# build the Haskell wrench from commit 10eb83a, e.g. in the ryukzak/wrench-builder image
python3 script/perf_compare.py path/to/haskell/wrench target/release/wrench test/performance/results.json
python3 script/perf_report.py test/performance/results.json test/performance
```

Raw numbers: [`results.json`](results.json).
""")
