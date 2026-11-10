//! One fresh process per observation, so CPU dispatch is initially uncached.
use std::hint::black_box;
use std::time::Instant;

use zmime_comparison::corpus;

fn main() {
    let mut args = std::env::args().skip(1);
    let engine = args.next().expect("cold ENGINE DECODED_BYTES");
    let size: usize = args
        .next()
        .expect("decoded bytes")
        .parse()
        .expect("integer");
    assert!((1..=16 * 1024 * 1024).contains(&size));
    assert!(engine == "zmime" || engine == "mail-parser-1.0.1");
    let decoded = corpus::payload(size);
    let input = corpus::base64(&decoded, false);
    let mut output = vec![0; decoded.len()];
    let mut reference = vec![0; decoded.len()];
    reference.clear();
    let mut operation = || {
        if engine == "zmime" {
            let length = zmime::decode_base64(black_box(&input), &mut output).unwrap();
            black_box(&output[..length]);
        } else {
            reference.clear();
            mail_parser::Encoding::Base64.decode_append(black_box(&input), &mut reference);
            black_box(reference.as_slice());
        }
    };
    let started = Instant::now();
    operation();
    let first_call_ns = started.elapsed().as_nanos();
    for _ in 0..32 {
        operation();
    }
    let iterations = (16 * 1024 * 1024 / size).clamp(4, 65536);
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let warmed_ns = started.elapsed().as_nanos() as f64 / iterations as f64;
    if engine == "zmime" {
        assert_eq!(output, decoded);
    } else {
        assert_eq!(reference, decoded);
    }
    println!("{engine},{},{first_call_ns},{warmed_ns:.3}", input.len());
}
