use std::hint::black_box;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use mail_parser::{Encoding, MessageBuffers};
use zmime::{Part, decode_base64, decode_quoted_printable};
use zmime_comparison::{assert_equivalent, baseline, consume, corpus, limits};

fn elapsed(iterations: usize, operation: &mut impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    start.elapsed()
}

struct Bench<'a> {
    samples: usize,
    target_ms: f64,
    filter: &'a str,
    out: io::BufWriter<io::Stdout>,
}

impl Bench<'_> {
    fn pair(
        &mut self,
        mode: &str,
        case: &str,
        bytes: usize,
        equivalent: bool,
        mut zmime: impl FnMut(),
        mut reference: impl FnMut(),
    ) {
        if !self.filter.is_empty() && !format!("{mode}/{case}").contains(self.filter) {
            return;
        }
        for _ in 0..32 {
            zmime();
            reference();
        }
        let pilot = 16;
        let z = elapsed(pilot, &mut zmime).as_secs_f64() / pilot as f64;
        let r = elapsed(pilot, &mut reference).as_secs_f64() / pilot as f64;
        let iterations = ((self.target_ms / 1000.0) / z.min(r).max(1e-9))
            .ceil()
            .clamp(1.0, 5_000_000.0) as usize;
        for sample in 0..self.samples {
            let (z, r) = if sample % 2 == 0 {
                (
                    elapsed(iterations, &mut zmime),
                    elapsed(iterations, &mut reference),
                )
            } else {
                let r = elapsed(iterations, &mut reference);
                (elapsed(iterations, &mut zmime), r)
            };
            for (engine, time) in [("zmime", z), ("mail-parser-1.0.1", r)] {
                writeln!(
                    self.out,
                    "{mode},{case},{bytes},{iterations},{sample},{engine},{:.3},{equivalent}",
                    time.as_nanos() as f64 / iterations as f64
                )
                .unwrap();
            }
        }
        self.out.flush().unwrap();
    }
}

fn main() {
    let mut samples = 9;
    let mut target_ms: f64 = 20.0;
    let mut filter = String::new();
    let mut corpus_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => samples = args.next().expect("sample count").parse().expect("integer"),
            "--target-ms" => {
                target_ms = args.next().expect("milliseconds").parse().expect("number")
            }
            "--filter" => filter = args.next().expect("filter"),
            "--dump-corpus" => corpus_dir = Some(args.next().expect("output directory")),
            "--help" => {
                println!(
                    "compare [--samples 9] [--target-ms 20] [--filter substring] [--dump-corpus directory]\nCSV on stdout; differential status on stderr. All fixtures are generated deterministically. --dump-corpus validates and writes .eml files without timing."
                );
                return;
            }
            _ => panic!("unknown argument: {arg}"),
        }
    }
    assert!((3..=100).contains(&samples));
    assert!(target_ms.is_finite() && target_ms > 0.0 && target_ms <= 1000.0);
    let parser = baseline();
    let cases = corpus::valid_cases();
    for case in &cases {
        assert_equivalent(case, &parser);
    }
    eprintln!(
        "Differential check passed for {} valid fixtures before timing.",
        cases.len()
    );
    if let Some(directory) = corpus_dir {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).expect("create fixture directory");
        for case in cases.iter().chain(corpus::malformed_cases().iter()) {
            std::fs::write(directory.join(format!("{}.eml", case.name)), &case.raw)
                .expect("write generated fixture");
        }
        return;
    }
    let mut bench = Bench {
        samples,
        target_ms,
        filter: &filter,
        out: io::BufWriter::new(io::stdout()),
    };
    writeln!(
        bench.out,
        "mode,case,input_bytes,iterations,sample,engine,ns_per_op,equivalent"
    )
    .unwrap();
    for case in &cases {
        let mut parameter_scratch = vec![0; limits().max_header_len];
        bench.pair(
            "headers",
            &case.name,
            case.raw.len(),
            true,
            || {
                let part = Part::parse_headers(black_box(&case.raw), limits()).unwrap();
                black_box(consume::zmime(black_box(&part), &mut parameter_scratch));
            },
            || {
                let message = parser.parse_headers(black_box(&case.raw)).unwrap();
                black_box(consume::reference(black_box(message.root_part())));
            },
        );
        let mut workspace = vec![Part::EMPTY; 512];
        let mut buffers = MessageBuffers::default();
        bench.pair(
            "structure_reuse",
            &case.name,
            case.raw.len(),
            true,
            || {
                let message =
                    zmime::Message::parse(black_box(&case.raw), &mut workspace, limits()).unwrap();
                let mut checksum = 0usize;
                for part in black_box(message.parts()) {
                    checksum = checksum.wrapping_add(consume::zmime(part, &mut parameter_scratch));
                }
                black_box(checksum);
            },
            || {
                let message = parser
                    .parse_with(black_box(&case.raw), &mut buffers)
                    .unwrap();
                let mut checksum = 0usize;
                for part in black_box(&message).parts() {
                    checksum = checksum.wrapping_add(consume::reference(part));
                }
                black_box(checksum);
                buffers = message.into_buffers();
            },
        );
        // This isolates the cost of mail-parser's allocating convenience API.
        // Do not print a speed ratio: zmime's caller workspace is reused here.
        bench.pair(
            "structure_fresh_reference",
            &case.name,
            case.raw.len(),
            false,
            || {
                let message =
                    zmime::Message::parse(black_box(&case.raw), &mut workspace, limits()).unwrap();
                black_box(message.parts());
            },
            || {
                let message = parser.parse(black_box(&case.raw)).unwrap();
                black_box(&message);
            },
        );
    }
    for length in [8, 1024, 64 * 1024, 1024 * 1024] {
        let decoded = corpus::payload(length);
        for folded in [false, true] {
            let input = corpus::base64(&decoded, folded);
            let mut output = vec![0; decoded.len()];
            let mut reference = Vec::with_capacity(decoded.len());
            assert_eq!(decode_base64(&input, &mut output).unwrap(), decoded.len());
            assert_eq!(output, decoded);
            Encoding::Base64.decode_append(&input, &mut reference);
            assert_eq!(reference, decoded);
            bench.pair(
                "transfer_base64",
                &format!("{length}_{}", if folded { "mime" } else { "flat" }),
                input.len(),
                true,
                || {
                    let length = decode_base64(black_box(&input), &mut output).unwrap();
                    black_box(&output[..length]);
                },
                || {
                    reference.clear();
                    Encoding::Base64.decode_append(black_box(&input), &mut reference);
                    black_box(reference.as_slice());
                },
            );
        }
        let text = b"A systems newsletter with MIME folding and ASCII fast paths.\r\n";
        let mut ascii = text.repeat(length.div_ceil(text.len()));
        ascii.truncate(length);
        while matches!(ascii.last(), Some(b' ' | b'\t' | b'\r')) {
            ascii.pop();
        }
        for (name, input, decoded) in [
            ("escaped", corpus::qp(&decoded), decoded),
            ("ascii", ascii.clone(), ascii),
        ] {
            let mut output = vec![0; input.len()];
            let mut reference = Vec::with_capacity(input.len());
            let count = decode_quoted_printable(&input, &mut output).unwrap();
            assert_eq!(output[..count], decoded);
            Encoding::QuotedPrintable.decode_append(&input, &mut reference);
            assert_eq!(reference, decoded);
            bench.pair(
                "transfer_qp",
                &format!("{length}_{name}"),
                input.len(),
                true,
                || {
                    let length = decode_quoted_printable(black_box(&input), &mut output).unwrap();
                    black_box(&output[..length]);
                },
                || {
                    reference.clear();
                    Encoding::QuotedPrintable.decode_append(black_box(&input), &mut reference);
                    black_box(reference.as_slice());
                },
            );
        }
    }
}
