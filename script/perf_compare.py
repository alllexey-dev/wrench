#!/usr/bin/env python3
"""Compare the Haskell and the Rust wrench binaries on the same workloads, write JSON.

usage: perf_compare.py HASKELL_BIN RUST_BIN OUT_JSON [HEAVY_RUNS] [LIGHT_RUNS]

Run from the repository root. Each case gets one warmup run, then the median
wall time and the max RSS over the measured runs are recorded.
Render the report with script/perf_report.py.
"""

import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

haskell, rust = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
out_json = Path(sys.argv[3])
heavy_runs = int(sys.argv[4]) if len(sys.argv) > 4 else 5
light_runs = int(sys.argv[5]) if len(sys.argv) > 5 else 30

PERF = "test/performance"


def examples():
    for d in sorted(p for p in Path("example").iterdir() if p.is_dir()):
        for conf in sorted(d.glob("*.yaml")):
            src = d / (conf.stem.split("-")[0] + ".s")
            if not src.exists():
                src = next(
                    (s for s in d.glob("*.s") if conf.stem.startswith(s.stem)), None
                )
            if src and conf.stem != "huge-report":
                yield f"{d.name}/{conf.stem}", [d.name, str(src), "-c", str(conf)]


GROUPS = [
    (
        "Simulation, heavy compute",
        heavy_runs,
        [
            (
                "f32a is_prime(2147483647)",
                ["f32a", f"{PERF}/program.s", "-c", f"{PERF}/conf.yaml"],
            ),
            *(
                (
                    f"risc-iv-32 {name}",
                    ["risc-iv-32", f"{PERF}/{name}.s", "-c", f"{PERF}/{name}.yaml"],
                )
                for name in ("fib_rec", "primes", "bubble_sort", "collatz")
            ),
        ],
    ),
    (
        "Simulation, heavy report (`slice: all`)",
        heavy_runs,
        [
            (
                "risc-iv-32 huge-report",
                [
                    "risc-iv-32",
                    "example/risc-iv-32/huge-report.s",
                    "-c",
                    "example/risc-iv-32/huge-report.yaml",
                ],
            )
        ],
    ),
    ("Simulation, small examples (startup dominated)", light_runs, list(examples())),
    (
        "Translation only (`-S`)",
        light_runs,
        [
            (
                "f32a is_prime",
                ["f32a", f"{PERF}/program.s", "-c", f"{PERF}/conf.yaml", "-S"],
            ),
            (
                "risc-iv-32 bubble_sort",
                [
                    "risc-iv-32",
                    f"{PERF}/bubble_sort.s",
                    "-c",
                    f"{PERF}/bubble_sort.yaml",
                    "-S",
                ],
            ),
        ],
    ),
]


def spawn(argv):
    start = time.perf_counter()
    proc = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, usage = os.wait4(proc.pid, 0)
    return (
        time.perf_counter() - start,
        usage.ru_maxrss / 1024,
        os.waitstatus_to_exitcode(status),
    )


# ru_maxrss of a child includes the RSS it inherited from this Python process before exec,
# so a value at this floor only means "no more than the floor"; the report marks such values.
RSS_FLOOR = spawn(["/bin/true"])[1]


def measure(binary, args, runs):
    isa, *rest = args
    argv = [binary, "--isa", isa, *rest]
    spawn(argv)
    samples = [spawn(argv) for _ in range(runs)]
    return (
        statistics.median(s[0] for s in samples) * 1000,
        max(s[1] for s in samples),
        {s[2] for s in samples},
    )


def version(binary):
    return subprocess.run(
        [binary, "--version"], check=False, capture_output=True, text=True
    ).stdout.strip()


def cpu_model():
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return platform.processor()


groups = []
for title, runs, cases in GROUPS:
    rows = []
    for name, args in cases:
        (ht, hm, hc), (rt, rm, rc) = (
            measure(haskell, args, runs),
            measure(rust, args, runs),
        )
        rows.append(
            {
                "name": name,
                "haskell_ms": ht,
                "rust_ms": rt,
                "haskell_rss_mb": hm,
                "rust_rss_mb": rm,
                "haskell_exit": sorted(hc),
                "rust_exit": sorted(rc),
            }
        )
        print(f"{name}: {ht:.1f} ms vs {rt:.1f} ms", file=sys.stderr, flush=True)
    groups.append({"title": title, "runs": runs, "cases": rows})

out_json.write_text(
    json.dumps(
        {
            "host": {
                "cpu": cpu_model(),
                "cpus": os.cpu_count(),
                "os": platform.platform(),
            },
            "haskell_version": version(haskell),
            "rust_version": version(rust),
            "rss_floor_mb": RSS_FLOOR,
            "groups": groups,
        },
        indent=2,
    )
    + "\n"
)
