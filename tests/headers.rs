use zmime::{Error, HeaderBlock, Limit, Limits};

#[test]
fn rfc5322_folding_preserves_raw_bytes_and_whitespace() {
    let input = b"SuBJeCt:  first\r\n\tsecond\n third\r\nX-Empty:\r\n\r\n\0binary\xff\r\n";
    let block = HeaderBlock::parse(input, Limits::DEFAULT).unwrap();
    assert_eq!(block.len(), 2);
    assert_eq!(block.body(), b"\0binary\xff\r\n");
    let subject = block.get(b"subject").unwrap();
    assert_eq!(subject.name(), b"SuBJeCt");
    assert_eq!(subject.raw_value(), b"  first\r\n\tsecond\n third");
    assert_eq!(subject.raw(), b"SuBJeCt:  first\r\n\tsecond\n third\r\n");
    let mut output = [0u8; 64];
    let n = subject.unfolded(&mut output).unwrap();
    assert_eq!(&output[..n], b"  first\tsecond third");
    assert_eq!(block.get(b"x-empty").unwrap().raw_value(), b"");
    assert!(block.get(b"absent").is_none());
}

#[test]
fn unfolding_capacity_failure_is_transactional() {
    let block = HeaderBlock::parse(b"X: a\r\n b\r\n\r\n", Limits::DEFAULT).unwrap();
    let header = block.get(b"X").unwrap();
    let mut out = [0xabu8; 3];
    assert_eq!(
        header.unfolded(&mut out),
        Err(Error::OutputTooSmall { needed: 4 })
    );
    assert_eq!(out, [0xab; 3]);
    let mut exact = [0u8; 4];
    assert_eq!(header.unfolded(&mut exact), Ok(4));
    assert_eq!(&exact, b" a b");
}

#[test]
fn duplicate_fields_remain_ordered_and_borrowed() {
    let input = b"X-ID: one\nX-ID: two\nX-ID: three\n\nbody";
    let block = HeaderBlock::parse(input, Limits::DEFAULT).unwrap();
    let fields: Vec<_> = block.headers().collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0].raw_value(), b" one");
    assert_eq!(fields[1].raw_value(), b" two");
    assert_eq!(fields[2].raw_value(), b" three");
    assert_eq!(fields[0].name().as_ptr(), input.as_ptr());
    assert_eq!(block.get(b"X-id"), Some(fields[0]));
    let raw: Vec<u8> = fields
        .iter()
        .flat_map(|field| field.raw().iter().copied())
        .collect();
    assert_eq!(raw, block.raw());
    let mut iter = block.headers();
    for _ in 0..3 {
        assert!(iter.next().is_some());
    }
    assert!(iter.next().is_none());
    assert!(iter.next().is_none());
}

#[test]
fn header_name_uses_printable_ascii_except_colon() {
    for byte in 0u8..=255 {
        let input = [byte, b':', b' ', b'v', b'\n', b'\n'];
        let accepted = HeaderBlock::parse(&input, Limits::DEFAULT).is_ok();
        // A leading LF itself ends the header block; subsequent bytes are body.
        let valid = (33..=57).contains(&byte) || (59..=126).contains(&byte) || byte == b'\n';
        assert_eq!(accepted, valid, "name byte {byte}");
    }
}

#[test]
fn forbidden_header_controls_and_orphan_continuations_fail() {
    for &byte in &[0, 1, 8, 11, 12, 14, 31, 127, b'\r'] {
        let input = [b'X', b':', b' ', byte, b'v', b'\n', b'\n'];
        assert_eq!(
            HeaderBlock::parse(&input, Limits::DEFAULT).unwrap_err(),
            Error::InvalidHeaderByte { offset: 3 }
        );
    }
    for input in [
        b" broken\n\n".as_slice(),
        b"\torphan\r\n\r\n",
        b": empty\n\n",
        b"Bad Name: x\n\n",
        b"NoColon\n\n",
    ] {
        assert!(matches!(
            HeaderBlock::parse(input, Limits::DEFAULT),
            Err(Error::InvalidHeader { offset: 0 })
        ));
    }
    assert!(HeaderBlock::parse(b"X: \tutf8\xff\xfe\n\n", Limits::DEFAULT).is_ok());
}

#[test]
fn truncation_needs_a_separator_but_never_parses_the_body() {
    for input in [
        b"X: value".as_slice(),
        b"X: value\n",
        b"X: value\r\n",
        b"\r",
    ] {
        assert!(matches!(
            HeaderBlock::parse(input, Limits::DEFAULT),
            Err(Error::MissingHeaderSeparator)
        ));
    }
    for input in [b"".as_slice(), b"\n", b"\r\n"] {
        assert!(HeaderBlock::parse(input, Limits::DEFAULT)
            .unwrap()
            .is_empty());
    }
    let block = HeaderBlock::parse(b"X: value\n\nnot a header\0\xff\r", Limits::DEFAULT).unwrap();
    assert_eq!(block.body(), b"not a header\0\xff\r");
}

#[test]
fn resource_limits_are_checked_at_their_edges() {
    let input = b"X: a\r\n b\r\n\r\nx";
    let limits = Limits {
        max_line_len: 4,
        max_headers: 1,
        max_header_len: 12,
        max_message_len: input.len(),
        ..Limits::DEFAULT
    };
    assert!(HeaderBlock::parse(input, limits).is_ok());
    for (limits, expected) in [
        (
            Limits {
                max_message_len: input.len() - 1,
                ..limits
            },
            Limit::MessageBytes,
        ),
        (
            Limits {
                max_header_len: 11,
                ..limits
            },
            Limit::HeaderBytes,
        ),
        (
            Limits {
                max_line_len: 3,
                ..limits
            },
            Limit::LineBytes,
        ),
        (
            Limits {
                max_headers: 0,
                ..limits
            },
            Limit::Headers,
        ),
    ] {
        assert!(
            matches!(HeaderBlock::parse(input, limits), Err(Error::LimitExceeded { limit, .. }) if limit == expected)
        );
    }
    assert!(matches!(
        HeaderBlock::parse(b"X: a\nY: b\n\n", limits),
        Err(Error::LimitExceeded {
            limit: Limit::Headers,
            ..
        })
    ));
    let no_limits = Limits {
        max_message_len: 0,
        max_header_len: 0,
        max_line_len: 0,
        max_headers: 0,
        max_parts: 0,
        max_depth: 0,
    };
    assert!(HeaderBlock::parse(b"", no_limits).is_ok());
    let bytes = vec![b'a'; if cfg!(miri) { 128 } else { 1 << 20 }];
    assert!(matches!(
        HeaderBlock::parse(
            &bytes,
            Limits {
                max_line_len: 8,
                ..Limits::DEFAULT
            }
        ),
        Err(Error::LimitExceeded {
            limit: Limit::LineBytes,
            offset: 10
        })
    ));
}

#[test]
fn random_header_inputs_terminate_and_keep_slices_inside_the_input() {
    let rounds = if cfg!(miri) { 32 } else { 4000 };
    let mut seed = 0xc6a4_a793_5bd1_e995u64;
    for round in 0..rounds {
        let length = round % 257;
        let mut input = Vec::with_capacity(length + 32);
        if round % 2 == 0 {
            input.extend_from_slice(b"X: ");
        }
        for _ in 0..length {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push(if round % 3 == 0 {
                32 + (seed % 95) as u8
            } else {
                seed as u8
            });
        }
        if round % 2 == 0 {
            input.extend_from_slice(b"\r\n\tcontinuation\n\nbody");
        }
        if let Ok(block) = HeaderBlock::parse(&input, Limits::DEFAULT) {
            assert!(block.raw().len() + block.body().len() <= input.len());
            let fields: Vec<_> = block.headers().collect();
            assert_eq!(fields.len(), block.len());
            assert_eq!(
                fields.iter().map(|field| field.raw().len()).sum::<usize>(),
                block.raw().len()
            );
            for field in fields {
                let mut out = vec![0u8; field.raw_value().len()];
                let n = field.unfolded(&mut out).unwrap();
                assert!(!out[..n].contains(&b'\r'));
                assert!(!out[..n].contains(&b'\n'));
            }
        }
    }
}
