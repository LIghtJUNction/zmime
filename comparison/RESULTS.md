# Measured performance against mail-parser 1.0.1

The 2× representative structure target is not met in this run: **1/8 CRLF structure cases reach 2×**. The tables retain every evaluated mail category and transfer workload, including regressions. No full-parser or complete character-set victory is claimed.

## Recorded environment

Measured **2026-11-10T16:56:55+08:00** (UTC+08:00), source commit `53ef4d5f42e5ab3017a9541c38872d02f26ad87f`. CPU: **INTEL(R) XEON(R) PLATINUM 8573C**; Linux 6.18.57; pinned CPU 8. Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Both libraries use thin LTO and one codegen unit. Each main row is the median of 9 alternating-order pairs targeting 20.0 ms per faster-engine sample.

The process ran in a shared cloud environment. Raw samples and their minimum/maximum ranges are preserved; small differences should be assessed against those ranges. The reference is exactly mail-parser 1.0.1 with its locked dependency graph and non-MIME headers configured as raw. The main benchmark validates the timed fixtures' observable results and matching consumer checksums before timing. Headers and reusable-structure timings include reading all MIME parameters, with quoted-string unescaping performed before the timer stops. Source, binaries, and exact generated mail bytes are SHA-256 identified in the environment records.

## CRLF parsing

Times are microseconds per operation (lower is better). Ratios are reference / zmime (above one favors zmime). Headers mode handles the root headers and MIME metadata; it does not scan the entire attachment body. Structure mode traverses the complete supported MIME tree with reused storage in both libraries and keeps transfer decoding lazy.

| Case | Message bytes | Headers: zmime µs | Headers: reference µs | Headers ratio | Structure: zmime µs | Structure: reference µs | Structure ratio |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Tiny | 16 | 0.082 | 0.206 | 2.511× | 0.087 | 0.290 | 3.339× |
| Ordinary | 1402 | 0.618 | 0.795 | 1.288× | 0.612 | 0.881 | 1.440× |
| One MiB attachment | 1435212 | 0.245 | 0.372 | 1.519× | 37.515 | 31.688 | 0.845× |
| Eight nested multiparts | 1263 | 0.219 | 0.319 | 1.458× | 5.062 | 4.270 | 0.844× |
| Newsletter | 65903 | 0.205 | 0.344 | 1.677× | 2.836 | 2.725 | 0.961× |
| Long folded headers | 70348 | 3.624 | 3.331 | 0.919× | 3.697 | 3.394 | 0.918× |
| 128 attachments | 54644 | 0.218 | 0.354 | 1.626× | 78.187 | 60.094 | 0.769× |
| Nested message/rfc822 | 221 | 0.222 | 0.364 | 1.638× | 0.766 | 0.974 | 1.272× |

## Transfer decoding

All decoded bytes agree. Both use preallocated output storage. Case numbers denote decoded-byte targets; input-byte counts include encoding overhead. Flat and MIME-wrapped Base64 are separate, as are escaped quoted-printable and natural ASCII text.

| Codec | Case | Input bytes | zmime µs | Reference µs | Ratio |
| --- | --- | --- | --- | --- | --- |
| base64 | 8_flat | 12 | 0.034 | 0.037 | 1.089× |
| base64 | 8_mime | 12 | 0.033 | 0.036 | 1.072× |
| qp | 8_escaped | 24 | 0.019 | 0.023 | 1.174× |
| qp | 8_ascii | 8 | 0.011 | 0.017 | 1.496× |
| base64 | 1024_flat | 1368 | 0.149 | 0.108 | 0.721× |
| base64 | 1024_mime | 1402 | 0.328 | 0.168 | 0.512× |
| qp | 1024_escaped | 3198 | 2.240 | 1.544 | 0.689× |
| qp | 1024_ascii | 1024 | 0.284 | 0.377 | 1.326× |
| base64 | 65536_flat | 87384 | 8.425 | 5.567 | 0.661× |
| base64 | 65536_mime | 89682 | 22.281 | 8.274 | 0.371× |
| qp | 65536_escaped | 204798 | 137.119 | 100.208 | 0.731× |
| qp | 65536_ascii | 65535 | 15.719 | 20.824 | 1.325× |
| base64 | 1048576_flat | 1398104 | 137.552 | 92.842 | 0.675× |
| base64 | 1048576_mime | 1434896 | 346.478 | 138.398 | 0.399× |
| qp | 1048576_escaped | 3276798 | 2449.330 | 1771.520 | 0.723× |
| qp | 1048576_ascii | 1048576 | 248.426 | 338.555 | 1.363× |

## SIMD ablation

The portable build disables zmime's SIMD feature, including scanning and transfer kernels. The reference keeps its own dispatch unchanged. The ablation ratio is portable zmime / default zmime; larger values indicate a faster default build. Reference timings from both runs are included to make environmental variation visible.

| Workload | Default zmime µs | Portable zmime µs | Ablation ratio | Reference in default run µs | Reference in portable run µs |
| --- | --- | --- | --- | --- | --- |
| structure_reuse / ordinary | 0.612 | 0.629 | 1.027× | 0.881 | 0.862 |
| structure_reuse / large_attachment | 37.515 | 175.171 | 4.669× | 31.688 | 33.020 |
| structure_reuse / long_headers | 3.697 | 15.184 | 4.107× | 3.394 | 3.572 |
| structure_reuse / many_attachments | 78.187 | 85.344 | 1.092× | 60.094 | 62.535 |
| transfer_base64 / 1048576_flat | 137.552 | 1527.841 | 11.107× | 92.842 | 99.181 |
| transfer_base64 / 1048576_mime | 346.478 | 1981.103 | 5.718× | 138.398 | 124.521 |
| transfer_qp / 1048576_escaped | 2449.330 | 2173.589 | 0.887× | 1771.520 | 1619.662 |
| transfer_qp / 1048576_ascii | 248.426 | 342.569 | 1.379× | 338.555 | 346.632 |

## First decode in a fresh process

These observations include uncached feature detection and initial code/data state, while process startup and buffer preparation are excluded. Each row gives the median across fresh processes and the warmed loop measured in those same processes. No cross-library cold speed ratio is calculated.

| Engine | Encoded bytes | First decode median µs | First decode min–max µs | Warmed loop median µs |
| --- | --- | --- | --- | --- |
| zmime | 1368 | 27.527 | 26.632–44.762 | 0.160 |
| mail-parser-1.0.1 | 1368 | 63.149 | 61.447–234.566 | 0.122 |
| zmime | 87384 | 33.546 | 32.763–38.403 | 8.084 |
| mail-parser-1.0.1 | 87384 | 90.616 | 88.774–180.524 | 4.741 |
| zmime | 1398104 | 490.228 | 402.724–661.146 | 130.272 |
| mail-parser-1.0.1 | 1398104 | 409.862 | 404.537–824.413 | 85.790 |

## LF parsing annex

| Case | Message bytes | Headers: zmime µs | Headers: reference µs | Headers ratio | Structure: zmime µs | Structure: reference µs | Structure ratio |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Tiny | 14 | 0.084 | 0.209 | 2.500× | 0.087 | 0.276 | 3.160× |
| Ordinary | 1351 | 0.590 | 0.789 | 1.337× | 0.596 | 0.835 | 1.400× |
| One MiB attachment | 1416801 | 0.226 | 0.334 | 1.478× | 36.396 | 33.292 | 0.915× |
| Eight nested multiparts | 1172 | 0.210 | 0.317 | 1.510× | 5.190 | 4.299 | 0.828× |
| Newsletter | 65013 | 0.206 | 0.348 | 1.694× | 2.610 | 2.725 | 1.044× |
| Long folded headers | 70244 | 3.682 | 3.302 | 0.897× | 3.714 | 3.206 | 0.863× |
| 128 attachments | 53487 | 0.203 | 0.331 | 1.633× | 77.077 | 62.172 | 0.807× |
| Nested message/rfc822 | 207 | 0.214 | 0.322 | 1.503× | 0.718 | 0.991 | 1.381× |

## Reproduction and scope

The full protocol, generated corpus, reference configuration, malformed-input policy differences, and commands are in [README.md](README.md). Malformed or truncated recovery is not timed as equivalent to strict rejection. The extra allocating-reference mode appears in the CSV with no speed ratio because storage lifecycles differ. The portable ablation measures the enabled kernel group, not an isolated instruction; these x86_64 results do not establish aarch64 or WASM throughput.

- Default SIMD: [samples](results/2026-11-10-reviewed-default/raw.csv), [summary and ranges](results/2026-11-10-reviewed-default/summary.csv), [cold observations](results/2026-11-10-reviewed-default/cold.csv), [environment](results/2026-11-10-reviewed-default/environment.json).
- Portable: [samples](results/2026-11-10-reviewed-portable/raw.csv), [summary and ranges](results/2026-11-10-reviewed-portable/summary.csv), [cold observations](results/2026-11-10-reviewed-portable/cold.csv), [environment](results/2026-11-10-reviewed-portable/environment.json).
