# zmime

A zero-dependency, `no_std` Rust parser for borrowed MIME message trees. Input is
`&[u8]`; callers supply part storage and decode buffers. The library uses only
`core`, performs no heap allocations, and never decodes an attachment during
structure parsing.

**MSRV: Rust 1.74.0.** Default builds use bounded SSE2 scans on x86_64, with
runtime-selected AVX2 scans and SSSE3/AVX2 Base64 kernels. Aarch64 uses NEON when
the target guarantees it. A private atomic caches CPU capability bits; the first
feature detection has a separate cold-start cost. Other targets use the portable
implementation. `default-features = false` selects portable code on every
architecture. No build script, C code, FFI, or runtime is required.

## Parse, inspect, and decode

```rust
use zmime::{Limits, Message, Part};

let input = b"Subject: Hello\r\n\
Content-Type: multipart/mixed; boundary=demo\r\n\r\n\
--demo\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello!\r\n\
--demo\r\nContent-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=sample.bin\r\n\
Content-Transfer-Encoding: base64\r\n\r\nAAEC/w==\r\n\
--demo--\r\n";

let mut workspace = [Part::EMPTY; 8];
let message = Message::parse(input, &mut workspace, Limits::default()).unwrap();
assert_eq!(message.parts().len(), 3); // container, text, attachment
assert_eq!(message.root().headers().get(b"subject").unwrap().raw_value(), b" Hello");

let attachment = message.attachments().next().unwrap();
assert_eq!(attachment.raw_body(), b"AAEC/w=="); // borrowed, still encoded
let mut decoded = [0u8; 4];
let written = attachment.decode_body(&mut decoded).unwrap();
assert_eq!(&decoded[..written], &[0, 1, 2, 255]);

let mut text = [0u8; 32];
assert_eq!(message.parts()[1].text(&mut text).unwrap(), "Hello!");
```

Use `HeaderBlock::parse` for validated raw fields only. Use
`Part::parse_headers` to also parse Content-Type, Content-Disposition, and
Content-Transfer-Encoding while leaving the complete body opaque. Header names
retain their original case; lookup is ASCII case insensitive. `raw_value()`
retains leading whitespace and folds, excluding the final physical line ending.
`Header::unfolded` removes fold CRLF/LF while retaining the following whitespace.

```rust
use zmime::decode_encoded_words;
let mut output = [0u8; 64];
let count = decode_encoded_words(b"=?UTF-8?Q?Caf=C3=A9?=", &mut output).unwrap();
assert_eq!(core::str::from_utf8(&output[..count]).unwrap(), "Café");
```

Run `cargo run --example inspect` for an executable example that unfolds and
decodes a subject, lists the MIME tree, reads its text, and decodes an attachment
on demand. All parsing and decoding buffers in the example have fixed capacity.

## Supported scope

| Area | Behavior |
| --- | --- |
| RFC 5322 framing | Raw fields, duplicate ordinary fields, folding, CRLF and LF; explicit errors for invalid names, controls, orphan folds and missing separators |
| MIME metadata | Type/subtype, disposition, ordered parameters, comments, quoted-pairs, default `text/plain` and multipart/digest rules |
| MIME structure | Multipart nesting, identity-encoded message/rfc822, preamble/epilogue, ancestor boundary recognition, borrowed raw entity/body slices |
| Base64 | MIME whitespace, strict alphabet, complete quartets, canonical padding and unused-bit validation |
| Quoted-printable | Hex escapes, CRLF/LF soft breaks, trailing physical-line whitespace removal, explicit malformed-input errors |
| RFC 2047 | B/Q encoded words, adjacent-word whitespace handling, UTF-8 / US-ASCII / ISO-8859-1 |
| Text conversion | UTF-8 / US-ASCII / ISO-8859-1; other charset labels return `UnsupportedCharset` |
| Delayed work | Transfer decoding and charset conversion occur only on explicit decode/text calls |

This is a format parser, with a strict documented subset. It does not parse
mailbox address lists or dates into semantic objects, convert HTML to text, or
provide a mail server. RFC 2231 extended/continued parameters are available as
raw attributes; requesting their decoded values returns `Unsupported`.
Transfer-encoded message/rfc822 requires two explicit steps: parse its headers,
decode its body into caller storage, then parse that buffer. Encoded multipart
containers are rejected. Missing multipart closing delimiters and duplicate
Content-Type/Content-Disposition/Content-Transfer-Encoding fields are errors.

Raw binary access does not imply text conversion. Unknown transfer encodings and
unsupported charset labels remain inspectable; decoding returns an explicit
error instead of substituting text. Invalid attachment encoding is detected
when that attachment is read, preserving lazy parsing.

## Resource and safety contract

`Limits` bounds total input size, header bytes per part, fields per part,
physical header line length, total part count, and nesting depth. Defaults are
64 MiB / 256 KiB / 256 fields / 64 KiB per line / 1024 parts / depth 32. The
root has depth zero. Nested comments have a fixed depth limit of 16. Applications
can tighten these values before accepting untrusted mail. Caller storage may be
smaller than the configured limits; exhaustion returns `WorkspaceTooSmall`.

Parsing uses an iterative state machine and one forward body scan. Active
ancestor boundaries are checked at line-start delimiter candidates. Worst-case
work is bounded by input length times configured nesting depth. No recursion or
input-sized stack array is used. A `Message` is returned only after the complete
tree validates. On failure, workspace contents may contain a partial tree;
decode outputs may contain a prefix and must be ignored after an error.

Only CPU feature detection and optimized scan/codec kernels use `unsafe`, with
local bounds and target feature proofs. The portable implementation is safe Rust. The test suite checks
optimized kernels against scalar references, random/truncated inputs and RFC
examples. CI runs Miri, platform tests and the independent differential suite.
See [safety notes](docs/safety.md) for the audit boundary and exact checks.

## Reproduce the comparison

The independent [comparison project](comparison/README.md) pins
`mail-parser = "=1.0.1"` and is excluded from the published crate, workspace,
and dependency graph. Its toolchain requirement belongs to that project only.
It compares validated headers, MIME structure with reused storage, and transfer
decoding separately. Each valid fixture must produce the same raw fields, MIME
parameters, parent relationships, body bytes, and decoded bytes before timing.
Malformed-input recovery policies are recorded without a speed ratio.

```sh
cargo test --locked
cargo test --locked --no-default-features
cargo +1.74.0 test --locked
cargo test --locked --manifest-path comparison/Cargo.toml
cargo run --release --locked --manifest-path comparison/Cargo.toml --bin compare -- \
  --samples 9 --target-ms 20 > results.csv
python3 comparison/summarize.py results.csv
```

Measurements are machine- and workload-specific. Results include scenarios in
which zmime is slower. The supported subset has no full-charset/full-mail
performance claim. The [measured results](comparison/RESULTS.md) contain raw CSV, machine metadata,
timing protocol, component ablations, and limits of the evidence.
In the recorded complete-metadata comparison, one of eight CRLF structure cases
reached 2×; the representative 2× target remains unmet. Tiny and ordinary mail
were faster, while large attachments, deep multipart trees, and several decoding
workloads remained slower than the pinned reference.
The [validation record](docs/validation.md) distinguishes executed local checks
from hosted CI and publication steps that still require a configured repository.

## CI and releases

CI covers Linux, Windows, macOS, native aarch64, stable, Rust 1.74.0, formatting,
clippy, docs, WASM/embedded builds, Miri, package checks and publish dry runs.
The published package has no normal, optional, build, dev, or target-specific
dependencies. Test programs and the independent comparison project can use std.
Portable library builds additionally forbid `unsafe` at compile time.

The manually triggered **Benchmark** workflow runs the same verified comparison
on x86_64 or native aarch64, with SIMD or portable kernels. It uploads paired
samples, summaries, and machine context as an Actions artifact. Shared hosted
runner measurements are observations, with no fixed speed threshold as a CI gate.

Run **Release** on the repository's default branch with one of `none`, `patch`,
`minor`, or `major`. `none` publishes the current version or completes an
interrupted publication; the other choices update the manifest and lockfiles.
The workflow tests the exact release commit, publishes using the repository
secret `CARGO_REGISTRY_TOKEN`, confirms the immutable registry archive, then
creates its Git tag and GitHub Release. It directly calls the reusable CI
workflow and does not depend on events generated by `GITHUB_TOKEN`.

Existing versions are reused only when archive checksums match. Conflicting
tags, different archive contents, yanked versions and untrusted branch/event
contexts stop the release. Re-running the same Actions run reuses its version
commit; a fresh recovery run should select `none`. See [release guide](docs/release.md).

## Standards and licensing

The implementation targets the documented subset of
[RFC 5322](https://www.rfc-editor.org/rfc/rfc5322),
[RFC 2045](https://www.rfc-editor.org/rfc/rfc2045),
[RFC 2046](https://www.rfc-editor.org/rfc/rfc2046),
[RFC 2047](https://www.rfc-editor.org/rfc/rfc2047), and
[RFC 2183](https://www.rfc-editor.org/rfc/rfc2183).
It preserves unsupported [RFC 2231](https://www.rfc-editor.org/rfc/rfc2231)
attributes without silently interpreting them.

Licensed under either Apache-2.0 or MIT, at your option.
