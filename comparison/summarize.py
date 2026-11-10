#!/usr/bin/env python3
"""Summarize paired raw samples without hiding regressions or dispersion."""

import csv
import math
import statistics
import sys
from collections import defaultdict


def main():
    samples = defaultdict(lambda: defaultdict(dict))
    metadata = {}
    with open(sys.argv[1], newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source):
            key = (row["mode"], row["case"])
            engine = row["engine"]
            sample = int(row["sample"])
            duration = float(row["ns_per_op"])
            if engine not in {"zmime", "mail-parser-1.0.1"} or row["equivalent"] not in {"true", "false"}:
                raise SystemExit(f"Invalid engine or equivalence flag in {key}")
            if not math.isfinite(duration) or duration <= 0 or int(row["iterations"]) <= 0:
                raise SystemExit(f"Invalid timing in {key}")
            if sample in samples[key][engine]:
                raise SystemExit(f"Duplicate sample {sample} for {engine} in {key}")
            if key in metadata and any(row[field] != metadata[key][field] for field in ("input_bytes", "equivalent")):
                raise SystemExit(f"Inconsistent workload metadata in {key}")
            samples[key][engine][sample] = (duration, int(row["iterations"]))
            metadata[key] = row
    if not samples:
        raise SystemExit("No benchmark samples found")
    for key, values in samples.items():
        if set(values) != {"zmime", "mail-parser-1.0.1"} or values["zmime"].keys() != values["mail-parser-1.0.1"].keys():
            raise SystemExit(f"Unpaired benchmark samples in {key}")
        if any(values["zmime"][sample][1] != values["mail-parser-1.0.1"][sample][1] for sample in values["zmime"]):
            raise SystemExit(f"Unequal paired iteration counts in {key}")
    output = csv.writer(sys.stdout)
    output.writerow([
        "mode", "case", "input_bytes", "samples", "zmime_median_ns",
        "reference_median_ns", "ratio_reference_over_zmime",
        "zmime_min_ns", "zmime_max_ns", "reference_min_ns", "reference_max_ns",
        "equivalent",
    ])
    for key, values in samples.items():
        z = [sample[0] for sample in values["zmime"].values()]
        r = [sample[0] for sample in values["mail-parser-1.0.1"].values()]
        row = metadata[key]
        zm = statistics.median(z)
        rm = statistics.median(r)
        equivalent = row["equivalent"] == "true"
        output.writerow([
            *key, row["input_bytes"], len(z), f"{zm:.3f}", f"{rm:.3f}",
            f"{rm / zm:.3f}" if equivalent else "",
            f"{min(z):.3f}", f"{max(z):.3f}", f"{min(r):.3f}", f"{max(r):.3f}",
            str(equivalent).lower(),
        ])


if __name__ == "__main__":
    main()
