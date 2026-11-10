# Safety and resource model

`zmime` parses untrusted mail into borrowed slices and caller-owned metadata. Its public API is safe Rust; the published crate unconditionally uses `#![no_std]`, imports only `core`, and declares Rust 1.74 as its minimum supported version. The root manifest has no normal, development, build, optional, or target-specific dependencies. The independent `comparison/` workspace uses its own toolchain and dependencies. These boundaries are checked by [check_contract.py](../scripts/check_contract.py) and the [CI workflow](../.github/workflows/ci.yml).

## Ownership and bounded work

The input, part array, and decoding output belong to the caller. `Message::parse` fills the supplied `&mut [Part]`; a successful `Message` borrows both that storage and the original `&[u8]`. Raw headers, bodies, attachments, preambles, and epilogues retain slices of the original input. Decoding writes only to the output slice supplied for that call. See [message.rs](../src/message.rs) and [decode.rs](../src/decode.rs).

MIME nesting uses parent indices and an iterative state machine. Every parent index refers to an earlier part, so walking toward the root terminates. Descending consumes a workspace entry and checks the depth and part budgets first. Boundary scanning advances through the body; each candidate is compared with open multipart ancestors. The worst-case work is proportional to input length times permitted depth, and nesting introduces no recursive call frames. Ancestor boundaries also take precedence while a child header block is read, including delimiter strings that resemble ordinary fields. [adversarial.rs](../tests/adversarial.rs) exercises these cases and the [RFC 2046 nesting rules](https://www.rfc-editor.org/rfc/rfc2046#section-5.1.2).

`Limits` bounds the amount of input the parser accepts before it consumes additional storage or scans an unbroken header line. Applications can lower the defaults to match their ingress limits. The header-byte budget includes the terminating separator; the physical-line budget excludes CRLF or LF.

| Resource | Default | Enforcement |
| --- | ---: | --- |
| Complete message | 64 MiB | Checked before parsing |
| Header block per part | 256 KiB | Bounds the scanner's search window |
| Logical fields per part | 256 | Continuation lines retain their original field |
| Physical header line | 64 KiB | Bounds each line search; set 998 for the RFC 5322 line-length restriction |
| Tree nodes | 1,024 | Includes containers and the root |
| Tree depth | 32 | Root depth is zero |
| Nested comments | 16 | Fixed iterative limit |
| Multipart boundary | 70 bytes | Fixed storage and syntax validation |

The supplied workspace and output slices are additional hard bounds. Insufficient part capacity returns `WorkspaceTooSmall`; decoding and unfolding return their documented output-capacity errors. A failed tree parse can leave partial metadata in the workspace, and a failed decoder can leave an output prefix. Use decoded bytes only after `Ok`; header unfolding and parameter unescaping additionally preserve output when capacity is insufficient. These behaviors are tested at exact and undersized capacities in [headers.rs](../tests/headers.rs), [mime.rs](../tests/mime.rs), and [decode.rs](../tests/decode.rs).

## Instruction selection and shared state

SIMD is an optional, default-enabled optimization. x86_64 uses its baseline SSE2 scanners and detects additional capabilities before selecting SSSE3 or AVX2 code. AVX2 selection checks both processor support and the operating system's enabled XMM/YMM state. AArch64 kernels require the target's NEON guarantee. Other targets and `default-features = false` use portable kernels. Rust requires the features used by an executed function to be supported by its platform; see the [Rust Reference](https://doc.rust-lang.org/reference/behavior-considered-undefined.html).

The portable library also declares `forbid(unsafe_code)` at compile time. This
makes the safe-Rust fallback an enforced source constraint. Test-only allocation
instrumentation belongs to its separate integration-test crate.

The x86 capability cache in [cpu.rs](../src/cpu.rs) is a single `AtomicU8`. It contains only instruction-capability bits and an initialization bit, with no pointers, mail data, output buffers, or parser state. Relaxed ordering suffices because no associated data is published with those bits; concurrent first calls can repeat feature detection and install an independently valid result. Each message's mutable parsing state remains in its own caller-provided workspace.

## Unsafe access arguments

Unsafe code is confined to CPU detection and private instruction kernels. Every unsafe site includes its local preconditions. The memory-access arguments below apply independently of whether an input is well-formed mail; malformed octets return to the scalar validator without relaxing those bounds.

| Kernel | Preconditions and exact access bounds |
| --- | --- |
| SSE2 / NEON byte search | A vector load requires 16 remaining input bytes. A group of four loads requires 64. Loads are unaligned, read-only, and originate from the live input slice. The tail uses safe portable code. |
| SSE2 / NEON header-control scan | Each vector load is guarded by 16 remaining input bytes. It returns the first forbidden byte and never writes memory. |
| AVX2 byte/control scan | Each load requires 32 available bytes; the byte-search group of four loads requires 128. Runtime detection establishes AVX2 and OS state support before the kernel is called. Both scanners are read-only and use bounded portable tails. |
| SSSE3 Base64 | Each vector read requires 16 input bytes and capacity for 12 output bytes. A fully valid block writes exactly eight plus four bytes. A prefix of three complete validated quartets writes exactly eight plus one byte, within the same 12-byte capacity guard. Other partial prefixes use a fixed 16-byte local array and a bounded slice copy. |
| AVX2 Base64 | Each vector read requires 32 input bytes and capacity for 24 output bytes. A fully valid block writes exactly 16 plus eight bytes. A prefix of three complete validated quartets writes exactly eight plus one byte, within the same 24-byte capacity guard. Other partial prefixes use a fixed 32-byte local array and copy only complete decoded quartets. Constant-table loads read complete 16-byte arrays. |
| NEON Base64 | Each interleaved load requires 64 input bytes and capacity for 48 output bytes. A fully valid block writes exactly 48 bytes; a partial prefix is packed into a fixed 48-byte local array and copied using bounded slices. The validity mask occupies a fixed 16-byte local array. |
| SSE2 / NEON quoted-printable literal scan | Each read requires 16 available bytes. Only printable literal runs are returned; escapes, line endings, and tails stay in the scalar decoder. |
| CPU detection | CPUID is available on x86_64. XGETBV is called only after OSXSAVE and AVX are advertised; AVX2 additionally requires XCR0 to enable XMM/YMM state and the AVX2 capability bit. These operations read processor state and do not access message memory. |

Input and output cannot overlap through these safe APIs because the decoder holds an immutable input slice and an exclusive mutable output slice simultaneously. SIMD emits only complete validated quartets, including valid prefixes before a fold, and may skip permitted MIME whitespace between those prefixes. Separate input-consumed and output-written counters keep whitespace from being counted as decoded data. Split quartets, padding, malformed bytes, and short tails return to the scalar state machine. Base64 padding bits are checked, and a short output receives an explicit capacity error. The implementation is in [scan.rs](../src/scan.rs) and [decode_simd.rs](../src/decode_simd.rs).

UTF-8 and ASCII text decoding can use these same bounded Base64 stores because
their decoded byte length is unchanged. Each produced prefix is passed through
the character validator before decoding continues. UTF-8 continuation state
survives vector boundaries and MIME folds. If output capacity is exhausted,
the scalar path still validates the remaining decoded text and counts the exact
required length. Latin-1 conversion uses the scalar sink because its UTF-8 output
can expand. Quoted-printable's hexadecimal lookup and eight-byte literal copies
use checked Rust array and slice operations; they add no unsafe sites.

## Evidence and reproduction

The following instruction paths have been executed under Miri during development. Miri checks the Rust memory and intrinsic rules along the tested paths; finite test coverage complements the local bounds arguments. Its scope is described in the [Miri documentation](https://github.com/rust-lang/miri).

| Path | Observed validation |
| --- | --- |
| Portable parsing and decoding | All 64 tests passed under strict-provenance Miri on the reviewed implementation, including headers, nested/adversarial input, decoding, and zero-allocation checks |
| SSE2 scanner | Reference-comparison tests passed under Miri on x86_64 |
| NEON scanner | All three scanner reference tests passed under strict-provenance Miri targeting aarch64 |
| AVX2 scanner | Three reference-comparison tests passed under strict-provenance Miri with `+avx2` |
| SSSE3 Base64 | All seven `base64_` tests passed with Miri configured for `+ssse3,-avx2` |
| AVX2 Base64 | All seven `base64_` tests passed with Miri configured for `+avx2` |
| NEON Base64 | Two vector tests and the MIME whitespace/line-length test passed under aarch64 Miri |
| SIMD text validation | All five `body_text_` tests passed under strict-provenance Miri for SSSE3, AVX2, and aarch64 NEON, including malformed text after output exhaustion |
| SSE2 / NEON QP scan | All byte values at each position 0–31 passed under Miri on both targets |
| CPU detection | Native calls exercise CPUID/XGETBV and the atomic cache. Miri selects capabilities from its target-feature configuration; it does not execute the host-detection path. |

The scanner tests compare every result with a scalar reference across lengths and alignments. Native header-control tests exhaust all 256 byte values; Miri uses control, whitespace, DEL, and high-bit boundary classes to bound interpreter time. Codec tests cover exact output bounds, tails, alignment, every byte in vector positions, deterministic generated data, malformed encodings, and truncation. `comparison/` independently checks the supported semantic subset against its pinned mail-parser version. [ci.yml](../.github/workflows/ci.yml) runs the portable, native, differential, and Miri gates.

Reproduce the principal safety checks from the repository root:

```sh
cargo +1.74.0 test --locked --all-targets
cargo test --locked --all-targets --no-default-features
cargo test --locked --manifest-path comparison/Cargo.toml
MIRIFLAGS=-Zmiri-strict-provenance cargo +nightly miri test --locked --lib --tests --no-default-features
MIRIFLAGS=-Zmiri-strict-provenance cargo +nightly miri test --locked --lib --target aarch64-unknown-linux-gnu scan::tests
RUSTFLAGS='-C target-feature=+avx2' MIRIFLAGS=-Zmiri-strict-provenance cargo +nightly miri test --locked --lib scan::tests
RUSTFLAGS='-C target-feature=+ssse3,-avx2' cargo +nightly miri test --locked --test decode base64_
RUSTFLAGS='-C target-feature=+avx2' cargo +nightly miri test --locked --test decode base64_
cargo +nightly miri test --locked --test decode --target aarch64-unknown-linux-gnu base64_
```

Native runtime dispatch selects the available host backend. Setting compiler flags to `+ssse3,-avx2` does not force native dispatch to ignore an AVX2-capable CPU; the Miri configuration above explicitly selects SSSE3 for interpretation.

## Interpretation boundaries

Structure parsing is lazy: it validates framing and MIME metadata while retaining encoded body slices. `Part::decode_body` performs transfer decoding on request; `Part::text` additionally converts UTF-8, US-ASCII, or ISO-8859-1. Other character sets return `UnsupportedCharset`. RFC 2047 B/Q words use the same explicit charset policy. Tests verify that an unread invalid Base64 attachment does not force decoding during tree construction.

The supported header subset preserves arbitrary field names and raw values without interpreting address lists or dates. RFC 2231 extended parameter bytes remain accessible, and requesting their decoded interpretation returns `Unsupported`. Transfer-encoded message containers require an explicit decode into a separate caller buffer before parsing that buffer; transfer-encoded multipart containers return an explicit error. These boundaries also constrain performance claims: the [comparison methodology](../comparison/README.md) defines equivalent work for each reported mode.

## Claim–evidence review

| Claim | Evidence | Status |
| --- | --- | --- |
| The published core requires no allocator or third-party dependency | Manifest, `#![no_std]`, source imports, [allocation-counting test](../tests/zero_alloc.rs), package/dependency contract check, embedded target build gate | Enforced by source and CI checks |
| Input cannot create recursive stack growth or unbounded metadata allocation | Iterative parent chain, caller workspace, explicit budgets, depth/workspace regression tests | Implemented and tested |
| SIMD reads and writes stay within the caller's slices | Per-site access proofs, exact-capacity tests, scalar comparisons, Miri runs listed above | Supported for the exercised kernels |
| Unsupported character sets are explicit | Charset dispatch and unsupported-charset regressions | Implemented and tested |
| Full MIME/charset equivalence is outside the stated subset | Explicit unsupported cases and mode-specific comparison eligibility | Scope documented |

Maintenance review: **contribution**—does the change improve this bounded parsing API? **clarity**—are ownership, offsets, and error behavior still explicit? **experimental strength**—does an optimization have reproducible measurements? **coverage**—were all affected backends and malformed tails exercised? **design soundness**—do every feature and memory-access precondition still hold? The relevant tests and bounds arguments must change with the implementation.
