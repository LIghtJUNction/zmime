use mail_parser::{Encoding, HeaderForm};
use zmime::{
    DecodeError, Message, Part, decode_base64, decode_encoded_words, decode_quoted_printable,
};
use zmime_comparison::{assert_equivalent, baseline, corpus, limits};

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn named_mail_fixtures_match_structure_headers_metadata_and_raw_bodies() {
    let parser = baseline();
    for case in corpus::valid_cases() {
        assert_equivalent(&case, &parser);
    }
}

#[test]
fn generated_valid_multipart_messages_match() {
    let parser = baseline();
    let mut state = 0x8c5b_149d_f772_30a1;
    for index in 0..300 {
        let lf = next(&mut state) & 1 != 0;
        let eol = if lf { "\n" } else { "\r\n" };
        let mut raw = format!("Subject: sample-{index}{eol}Content-Type: multipart/mixed; boundary=case-{index}{eol}{eol}Preamble{eol}").into_bytes();
        for child in 0..(next(&mut state) % 12 + 1) {
            let data = corpus::payload((next(&mut state) % 2048) as usize);
            raw.extend_from_slice(format!("--case-{index}{eol}Content-Type: application/octet-stream; name=\"file-{child}.bin\"{eol}Content-Disposition: attachment; filename=\"file-{child}.bin\"{eol}Content-Transfer-Encoding: base64{eol}{eol}").as_bytes());
            raw.extend_from_slice(&corpus::base64(&data, true));
            raw.extend_from_slice(eol.as_bytes());
        }
        raw.extend_from_slice(format!("--case-{index}--{eol}Epilogue{eol}").as_bytes());
        assert_equivalent(
            &corpus::Case {
                name: format!("generated-{index}"),
                raw,
            },
            &parser,
        );
    }
}

#[test]
fn generated_nested_trees_and_quoted_parameters_match() {
    let parser = baseline();
    let mut state = 0xd8a2_157c_b392_0047;
    for index in 0..96 {
        let eol = if next(&mut state) & 1 == 0 {
            "\r\n"
        } else {
            "\n"
        };
        let mut name = String::new();
        for _ in 0..(next(&mut state) % 63 + 1) {
            let byte = b"abcXYZ()\\\"_- ."[(next(&mut state) % 14) as usize];
            if byte == b'\\' || byte == b'"' {
                name.push('\\');
            }
            name.push(char::from(byte));
        }
        let data = corpus::payload((next(&mut state) % 256) as usize);
        let mut raw = format!("Content-Type: application/octet-stream;{eol}\tname=\"{name}\"{eol}Content-Disposition: attachment (comment);{eol} filename=\"{name}\"{eol}Content-Transfer-Encoding: base64{eol}{eol}").into_bytes();
        raw.extend_from_slice(&corpus::base64(&data, index & 1 == 0));
        if index % 3 == 0 {
            let mut wrapper = format!("Content-Type: message/rfc822{eol}{eol}").into_bytes();
            wrapper.extend_from_slice(&raw);
            raw = wrapper;
        }
        for depth in 0..(next(&mut state) % 6 + 1) {
            let boundary = format!("case-{index}-{depth}");
            let mut outer = format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"{eol}{eol}preamble{eol}--{boundary}{eol}Content-Type: text/plain{eol}{eol}sibling{eol}--{boundary}{eol}").into_bytes();
            outer.extend_from_slice(&raw);
            outer.extend_from_slice(format!("{eol}--{boundary}--{eol}epilogue").as_bytes());
            raw = outer;
        }
        assert_equivalent(
            &corpus::Case {
                name: format!("nested-quoted-{index}"),
                raw,
            },
            &parser,
        );
    }
}

#[test]
fn folded_metadata_comments_boundary_prefixes_and_digest_match() {
    let parser = baseline();
    for (name, raw) in [
        ("folded_metadata", &b"Content-Type: (mime) multipart/mixed;\r\n boundary=\"a b\"\r\n\r\n--a b\r\nContent-Type: TEXT/PLAIN; charset=\"US-ASCII\"\r\nContent-Disposition: inline; filename=\"a\\\"b.txt\"\r\n\r\nbody\r\n--a b-- \t\r\n"[..]),
        ("boundary_prefix", &b"Content-Type: multipart/mixed; boundary=a\r\n\r\n--a\r\nContent-Type: text/plain\r\n\r\n--ab\r\nbody--a\r\n--a-\r\n--a--\r\n"[..]),
        ("digest_default", &b"Content-Type: multipart/digest; boundary=d\r\n\r\n--d\r\n\r\nFrom: Bob <bob@example.test>\r\nSubject: Nested default\r\n\r\nbody\r\n--d--\r\n"[..]),
        ("empty_leaf", &b"Content-Type: multipart/mixed; boundary=e\r\n\r\n--e\r\nX-Empty: yes\r\n\r\n\r\n--e--\r\n"[..]),
    ] {
        assert_equivalent(&corpus::Case { name: name.into(), raw: raw.to_vec() }, &parser);
    }
}

#[test]
fn arbitrary_bytes_roundtrip_through_both_transfer_decoders() {
    for length in (0..512).chain([1024, 4096, 65536]) {
        let data = corpus::payload(length);
        let mut output = vec![0; data.len()];
        for wrap in [false, true] {
            let encoded = corpus::base64(&data, wrap);
            let length = decode_base64(&encoded, &mut output).expect("valid generated Base64");
            assert_eq!(&output[..length], data);
            let (reference, malformed) = Encoding::Base64.decode_checked(&encoded);
            assert!(!malformed);
            assert_eq!(reference.as_ref(), data);
            if !data.is_empty() {
                assert_eq!(
                    decode_base64(&encoded, &mut output[..data.len() - 1]),
                    Err(DecodeError::OutputTooSmall { needed: data.len() })
                );
            }
        }
        let encoded = corpus::qp(&data);
        let length = decode_quoted_printable(&encoded, &mut output)
            .expect("valid generated quoted-printable");
        assert_eq!(&output[..length], data);
        let (reference, malformed) = Encoding::QuotedPrintable.decode_checked(&encoded);
        assert!(!malformed);
        assert_eq!(reference.as_ref(), data);
    }
}

#[test]
fn encoded_word_supported_charsets_match() {
    for input in [
        &b"=?UTF-8?B?5L2g5aW9?="[..],
        &b"=?utf-8?Q?hello_world?="[..],
        &b"=?iso-8859-1?Q?caf=E9?="[..],
        &b"=?us-ascii?B?aGVsbG8=?="[..],
        &b"Re: =?UTF-8?Q?hello?= =?UTF-8?Q?_world?="[..],
        &b"=?utf-8?B?YWJj?=\r\n\t=?utf-8?Q?def?="[..],
    ] {
        let mut output = vec![0; input.len() * 4];
        let length = decode_encoded_words(input, &mut output).unwrap();
        let parsed = HeaderForm::Text.parse(input);
        assert_eq!(
            &output[..length],
            parsed.value().as_text().unwrap().as_bytes()
        );
    }
}

#[test]
fn literal_latin1_and_reference_windows1252_policy_are_explicitly_different() {
    let input = b"=?iso-8859-1?Q?=80?=";
    let mut output = [0; 16];
    let length = decode_encoded_words(input, &mut output).unwrap();
    assert_eq!(&output[..length], "\u{80}".as_bytes());
    // mail-parser intentionally uses a web-compatible Windows-1252 mapping
    // for this label; zmime implements literal ISO-8859-1. This is a policy
    // comparison, not an equality assertion or timed speed-ratio workload.
    assert_eq!(HeaderForm::Text.parse(input).value().as_text(), Some("€"));
}

#[test]
fn strict_rejection_and_tolerant_recovery_are_not_timed_as_equivalent() {
    let parser = baseline();
    let mut output = vec![0; 4096];
    for case in corpus::malformed_cases() {
        let mut workspace = vec![Part::EMPTY; 512];
        let ours = Message::parse(&case.raw, &mut workspace, limits());
        let reference = parser.parse(&case.raw);
        if case.name.contains("base64") || case.name.contains("qp") {
            let ours = ours.expect("transfer errors are lazy");
            let root = &ours.parts()[0];
            let decoded = root
                .transfer_encoding()
                .decode(root.raw_body(), &mut output);
            assert!(
                decoded.is_err(),
                "{}: strict decoder must reject",
                case.name
            );
            let reference = reference.expect("reference recovery");
            let (reference_decoded, problems) = reference.root_part().decoded_checked();
            if case.name == "truncated_base64" {
                // mail-parser's MIME policy accepts omitted padding without a
                // problem flag. zmime deliberately requires complete quartets.
                assert_eq!(reference_decoded.as_ref(), b"Hello");
                assert!(problems.is_empty());
            } else if case.name == "truncated_qp" {
                // The reference also permits a final soft-break marker at EOF.
                assert_eq!(reference_decoded.as_ref(), b"abc");
                assert!(problems.is_empty());
            } else {
                assert!(
                    !problems.is_empty(),
                    "{}: reference reports recovery",
                    case.name
                );
            }
        } else if case.name != "empty" {
            assert!(
                ours.is_err(),
                "{}: malformed structure must fail",
                case.name
            );
        }
    }
}

#[test]
fn random_bytes_and_mutated_messages_remain_bounded() {
    let parser = baseline();
    let mut state = 0x7e3d_5924_c8b1_a06f;
    let mut output = vec![0; 4096];
    for _ in 0..6000 {
        let length = (next(&mut state) % 1024) as usize;
        let raw: Vec<u8> = (0..length).map(|_| next(&mut state) as u8).collect();
        let mut workspace = vec![Part::EMPTY; 512];
        let _ = Message::parse(&raw, &mut workspace, limits());
        let _ = Part::parse_headers(&raw, limits());
        let _ = parser.parse(&raw);
        let _ = decode_base64(&raw, &mut output);
        let _ = decode_quoted_printable(&raw, &mut output);
        let _ = decode_encoded_words(&raw, &mut output);
    }
    let cases = corpus::valid_cases();
    for _ in 0..1200 {
        let case = &cases[(next(&mut state) % cases.len() as u64) as usize];
        let mut raw = case.raw[..case.raw.len().min(4096)].to_vec();
        for _ in 0..8 {
            if !raw.is_empty() {
                let at = next(&mut state) as usize % raw.len();
                raw[at] = next(&mut state) as u8;
            }
        }
        let mut workspace = vec![Part::EMPTY; 512];
        let _ = Message::parse(&raw, &mut workspace, limits());
        let _ = parser.parse(&raw);
    }
}
