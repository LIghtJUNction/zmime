#!/usr/bin/env python3
"""Render a complete measured report from default and portable run directories."""

import csv
import datetime
import json
from pathlib import Path
import statistics
import sys


CASES = [
    ("tiny", "Tiny"),
    ("ordinary", "Ordinary"),
    ("large_attachment", "One MiB attachment"),
    ("nested_multipart", "Eight nested multiparts"),
    ("newsletter", "Newsletter"),
    ("long_headers", "Long folded headers"),
    ("many_attachments", "128 attachments"),
    ("nested_message", "Nested message/rfc822"),
]


def read(directory):
    context = json.loads((directory / "environment.json").read_text())
    with (directory / "summary.csv").open(newline="") as source:
        rows = {(row["mode"], row["case"]): row for row in csv.DictReader(source)}
    with (directory / "cold.csv").open(newline="") as source:
        cold = list(csv.DictReader(source))
    return context, rows, cold


def us(value):
    return f"{float(value) / 1000:.3f}"


def table(headers, rows):
    print("| " + " | ".join(headers) + " |")
    print("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        print("| " + " | ".join(str(value) for value in row) + " |")
    print()


def mail_table(rows, suffix):
    values = []
    for key, name in CASES:
        h = rows[("headers", key + suffix)]
        s = rows[("structure_reuse", key + suffix)]
        values.append([
            name, h["input_bytes"], us(h["zmime_median_ns"]), us(h["reference_median_ns"]),
            h["ratio_reference_over_zmime"] + "×", us(s["zmime_median_ns"]),
            us(s["reference_median_ns"]), s["ratio_reference_over_zmime"] + "×",
        ])
    table(["Case", "Message bytes", "Headers: zmime µs", "Headers: reference µs", "Headers ratio", "Structure: zmime µs", "Structure: reference µs", "Structure ratio"], values)


def main():
    default_dir, portable_dir = map(Path, sys.argv[1:3])
    context, rows, cold = read(default_dir)
    portable_context, portable, _ = read(portable_dir)
    if any(recorded.get("protocol_version") != 2 for recorded in (context, portable_context)):
        raise SystemExit("Use reviewed protocol 2 measurements; older runs did not time complete MIME metadata consumption")
    if context["git_commit"] != portable_context["git_commit"]:
        raise SystemExit("The default and portable runs must use the same source commit")
    if context["source_sha256"] != portable_context["source_sha256"]:
        raise SystemExit("The default and portable runs must have identical source hashes")
    if context["fixture_sha256"] != portable_context["fixture_sha256"]:
        raise SystemExit("The default and portable runs must have identical fixture hashes")
    for recorded in (context, portable_context):
        if recorded.get("source_unchanged") is not True or recorded.get("binaries_unchanged") is not True:
            raise SystemExit("A run changed source or binaries during measurement")
    run_time = datetime.datetime.fromisoformat(context["started_utc"]).astimezone(datetime.timezone(datetime.timedelta(hours=8)))
    successes = sum(
        float(rows[("structure_reuse", key)]["reference_median_ns"])
        >= 2 * float(rows[("structure_reuse", key)]["zmime_median_ns"])
        for key, _ in CASES
    )
    print("# Measured performance against mail-parser 1.0.1\n")
    print(f"The 2× representative structure target is {'met' if successes == len(CASES) else 'not met'} in this run: **{successes}/{len(CASES)} CRLF structure cases reach 2×**. "
          "The tables retain every evaluated mail category and transfer workload, including regressions. No full-parser or complete character-set victory is claimed.\n")
    print("## Recorded environment\n")
    print(f"Measured **{run_time.isoformat(timespec='seconds')}** (UTC+08:00), source commit `{context['git_commit']}`. "
          f"CPU: **{context['cpu'].get('model name', context['architecture'])}**; {context['system']} {context['kernel']}; pinned CPU {context['selected_cpu']}. "
          f"Compiler: `{context['rustc'].splitlines()[0]}`. Both libraries use thin LTO and one codegen unit. "
          f"Each main row is the median of {context['samples']} alternating-order pairs targeting {context['target_ms']} ms per faster-engine sample.\n")
    if context["samples"] != portable_context["samples"] or context["target_ms"] != portable_context["target_ms"]:
        print(f"The portable ablation uses {portable_context['samples']} pairs targeting {portable_context['target_ms']} ms; its full samples are retained separately.\n")
    print("The process ran in a shared cloud environment. Raw samples and their minimum/maximum ranges are preserved; small differences should be assessed against those ranges. "
          "The reference is exactly mail-parser 1.0.1 with its locked dependency graph and non-MIME headers configured as raw. "
          "The main benchmark validates the timed fixtures' observable results and matching consumer checksums before timing. "
          "Headers and reusable-structure timings include reading all MIME parameters, with quoted-string unescaping performed before the timer stops. "
          "Source, binaries, and exact generated mail bytes are SHA-256 identified in the environment records.\n")
    if context["tracked_modifications"] or portable_context["tracked_modifications"]:
        print("Tracked changes were present during collection; inspect the environment records for the exact paths.\n")
    print("## CRLF parsing\n")
    print("Times are microseconds per operation (lower is better). Ratios are reference / zmime (above one favors zmime). "
          "Headers mode handles the root headers and MIME metadata; it does not scan the entire attachment body. "
          "Structure mode traverses the complete supported MIME tree with reused storage in both libraries and keeps transfer decoding lazy.\n")
    mail_table(rows, "")
    print("## Transfer decoding\n")
    print("All decoded bytes agree. Both use preallocated output storage. Case numbers denote decoded-byte targets; input-byte counts include encoding overhead. "
          "Flat and MIME-wrapped Base64 are separate, as are escaped quoted-printable and natural ASCII text.\n")
    transfer = []
    for (mode, case), row in rows.items():
        if mode.startswith("transfer_"):
            transfer.append([mode.removeprefix("transfer_"), case, row["input_bytes"], us(row["zmime_median_ns"]), us(row["reference_median_ns"]), row["ratio_reference_over_zmime"] + "×"])
    table(["Codec", "Case", "Input bytes", "zmime µs", "Reference µs", "Ratio"], transfer)
    print("## SIMD ablation\n")
    print("The portable build disables zmime's SIMD feature, including scanning and transfer kernels. The reference keeps its own dispatch unchanged. "
          "The ablation ratio is portable zmime / default zmime; larger values indicate a faster default build. "
          "Reference timings from both runs are included to make environmental variation visible.\n")
    ablation = []
    for key in [
        ("structure_reuse", "ordinary"), ("structure_reuse", "large_attachment"),
        ("structure_reuse", "long_headers"), ("structure_reuse", "many_attachments"),
        ("transfer_base64", "1048576_flat"), ("transfer_base64", "1048576_mime"),
        ("transfer_qp", "1048576_escaped"), ("transfer_qp", "1048576_ascii"),
    ]:
        default, scalar = rows[key], portable[key]
        ratio = float(scalar["zmime_median_ns"]) / float(default["zmime_median_ns"])
        ablation.append([" / ".join(key), us(default["zmime_median_ns"]), us(scalar["zmime_median_ns"]), f"{ratio:.3f}×", us(default["reference_median_ns"]), us(scalar["reference_median_ns"])])
    table(["Workload", "Default zmime µs", "Portable zmime µs", "Ablation ratio", "Reference in default run µs", "Reference in portable run µs"], ablation)
    print("## First decode in a fresh process\n")
    print("These observations include uncached feature detection and initial code/data state, while process startup and buffer preparation are excluded. "
          "Each row gives the median across fresh processes and the warmed loop measured in those same processes. No cross-library cold speed ratio is calculated.\n")
    groups = {}
    for row in cold:
        groups.setdefault((row["engine"], row["input_bytes"]), []).append(row)
    table(["Engine", "Encoded bytes", "First decode median µs", "First decode min–max µs", "Warmed loop median µs"], [
        [engine, size, us(statistics.median(float(row["first_call_ns"]) for row in values)),
         f"{us(min(float(row['first_call_ns']) for row in values))}–{us(max(float(row['first_call_ns']) for row in values))}",
         us(statistics.median(float(row["warmed_ns"]) for row in values))]
        for (engine, size), values in groups.items()
    ])
    print("## LF parsing annex\n")
    mail_table(rows, "_lf")
    print("## Reproduction and scope\n")
    print("The full protocol, generated corpus, reference configuration, malformed-input policy differences, and commands are in [README.md](README.md). "
          "Malformed or truncated recovery is not timed as equivalent to strict rejection. The extra allocating-reference mode appears in the CSV with no speed ratio because storage lifecycles differ. "
          "The portable ablation measures the enabled kernel group, not an isolated instruction; these x86_64 results do not establish aarch64 or WASM throughput.\n")
    for name, directory in [("Default SIMD", default_dir), ("Portable", portable_dir)]:
        relative = directory.name
        print(f"- {name}: [samples](results/{relative}/raw.csv), [summary and ranges](results/{relative}/summary.csv), "
              f"[cold observations](results/{relative}/cold.csv), [environment](results/{relative}/environment.json).")


if __name__ == "__main__":
    main()
