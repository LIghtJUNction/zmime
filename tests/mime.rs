use zmime::{ContentDisposition, ContentType, Error, Limit, ParameterValue, Unsupported};

fn decoded(value: ParameterValue<'_>) -> Vec<u8> {
    let mut out = vec![0u8; value.raw().len()];
    let n = value.decode(&mut out).unwrap();
    out.truncate(n);
    out
}

#[test]
fn content_type_understands_comments_folds_and_quoted_pairs() {
    let raw = b" (comment (nested)) TeXt (type) /\r\n\tPlAiN ; CHARSET = (x) \"Us-AsCiI\"; name=\"a\\\";b\\\\c\r\n d.txt\" (end)";
    let content_type = ContentType::parse(raw).unwrap();
    assert!(content_type.is(b"text", b"plain"));
    assert!(!content_type.is_multipart());
    assert_eq!(content_type.top_level(), b"TeXt");
    assert_eq!(content_type.subtype(), b"PlAiN");
    let charset = content_type.parameter(b"charset").unwrap().unwrap();
    assert_eq!(charset.name(), b"CHARSET");
    assert!(charset.value().eq_ignore_ascii_case(b"us-ascii").unwrap());
    assert!(!charset.value().eq_ignore_ascii_case(b"us").unwrap());
    assert!(
        !charset
            .value()
            .eq_ignore_ascii_case(b"us-ascii-extra")
            .unwrap()
    );
    let name = content_type.parameter(b"NAME").unwrap().unwrap().value();
    assert!(name.is_quoted());
    assert_eq!(name.raw(), b"a\\\";b\\\\c\r\n d.txt");
    assert_eq!(decoded(name), b"a\";b\\c d.txt");
    assert!(content_type.parameter(b"missing").unwrap().is_none());
}

#[test]
fn dispositions_keep_parameter_order_case_and_duplicates() {
    let disposition = ContentDisposition::parse(
        b"AtTaChMeNt; filename=first.txt; filename=\"second.txt\"; size=123",
    )
    .unwrap();
    assert!(disposition.is_attachment());
    assert_eq!(disposition.kind(), b"AtTaChMeNt");
    let params: Vec<_> = disposition.parameters().map(Result::unwrap).collect();
    assert_eq!(params.len(), 3);
    assert_eq!(params[0].value().raw(), b"first.txt");
    assert_eq!(params[1].value().raw(), b"second.txt");
    assert_eq!(
        disposition
            .parameter(b"FILENAME")
            .unwrap()
            .unwrap()
            .value()
            .raw(),
        b"first.txt"
    );
    assert!(
        !ContentDisposition::parse(b"inline (display)")
            .unwrap()
            .is_attachment()
    );
    let mut iter = disposition.parameters();
    for _ in 0..3 {
        assert!(iter.next().unwrap().is_ok());
    }
    assert!(iter.next().is_none());
    assert!(iter.next().is_none());
}

#[test]
fn empty_quoted_values_and_exact_output_capacities() {
    let content_type =
        ContentType::parse(b"application/octet-stream; name=\"\"; x=\"a\\ b\"").unwrap();
    let empty = content_type.parameter(b"name").unwrap().unwrap().value();
    assert_eq!(empty.decode(&mut []), Ok(0));
    assert!(empty.eq_ignore_ascii_case(b"").unwrap());
    let value = content_type.parameter(b"x").unwrap().unwrap().value();
    let mut out = [0x5au8; 2];
    assert_eq!(
        value.decode(&mut out),
        Err(Error::OutputTooSmall { needed: 3 })
    );
    assert_eq!(out, [0x5a; 2]);
    assert_eq!(decoded(value), b"a b");
}

#[test]
fn rfc2231_extended_parameters_are_preserved_but_explicitly_unsupported() {
    let disposition = ContentDisposition::parse(
        b"attachment; filename*=utf-8''caf%C3%A9.txt; filename*0=abc; filename*1=def",
    )
    .unwrap();
    for parameter in disposition.parameters() {
        let value = parameter.unwrap().value();
        assert!(value.is_extended());
        let mut output = [0xacu8; 64];
        assert_eq!(
            value.decode(&mut output),
            Err(Error::Unsupported(Unsupported::ExtendedParameter))
        );
        assert_eq!(
            value.eq_ignore_ascii_case(b"text"),
            Err(Error::Unsupported(Unsupported::ExtendedParameter))
        );
        assert_eq!(output, [0xac; 64]);
    }
}

#[test]
fn comment_depth_is_bounded_without_recursion() {
    for depth in [1, 16, 17, 128] {
        let value = format!("{}x{}text/plain", "(".repeat(depth), ")".repeat(depth));
        let result = ContentType::parse(value.as_bytes());
        if depth <= 16 {
            assert!(result.unwrap().is(b"text", b"plain"));
        } else {
            assert!(matches!(
                result,
                Err(Error::LimitExceeded {
                    limit: Limit::CommentDepth,
                    offset: 16
                })
            ));
        }
    }
    let value = format!("text/plain {}x{}", "(".repeat(17), ")".repeat(17));
    assert!(matches!(
        ContentType::parse(value.as_bytes()),
        Err(Error::LimitExceeded {
            limit: Limit::CommentDepth,
            offset: 27
        })
    ));
}

#[test]
fn invalid_structured_field_syntax_is_rejected() {
    for value in [
        b"".as_slice(),
        b"text",
        b"text/",
        b"/plain",
        b"text/plain extra",
        b"text / plain ;",
        b"text/plain; charset",
        b"text/plain; charset=",
        b"text/plain; charset=\"unterminated",
        b"text/plain; charset=\"bad\\",
        b"text/plain; charset=\"bad\0\"",
        b"text/plain; charset=us ascii",
        b"text/plain; charset=\"line\rbreak\"",
        b"text/plain; charset=\"line\nbreak\"",
        b"text/plain (unclosed",
        b"text/plain )",
        b"text/plain (bad\\\0)",
        b"text/plain; name=abc/def",
        b"text/plain; name=abc=def",
        b"text/plain; =value",
    ] {
        assert!(
            ContentType::parse(value).is_err(),
            "accepted malformed value {value:?}"
        );
    }
    for value in [
        b"".as_slice(),
        b"\"attachment\"",
        b"attachment inline",
        b"attachment;",
        b"attachment; filename",
    ] {
        assert!(ContentDisposition::parse(value).is_err());
    }
}

#[test]
fn structured_error_offsets_are_relative_to_the_complete_input() {
    assert_eq!(
        ContentType::parse(b"text/plain; name=").unwrap_err(),
        Error::InvalidMime { offset: 17 }
    );
    assert_eq!(
        ContentDisposition::parse(b"attachment; name=").unwrap_err(),
        Error::InvalidMime { offset: 17 }
    );
    assert_eq!(
        ContentType::parse(b"text/plain; name=\"bad\rbreak\"").unwrap_err(),
        Error::InvalidMime { offset: 22 }
    );
}

#[test]
fn defaults_follow_mime_and_multipart_digest_rules() {
    assert!(ContentType::TEXT_PLAIN.is(b"text", b"plain"));
    assert!(ContentType::MESSAGE_RFC822.is(b"message", b"rfc822"));
    assert!(
        ContentType::parse(b"MULTIPART/alternative; boundary=x")
            .unwrap()
            .is_multipart()
    );
}

#[test]
fn random_and_truncated_values_terminate_and_preserve_valid_parameters() {
    let rounds = if cfg!(miri) { 32 } else { 4000 };
    let mut seed = 0x243f_6a88_85a3_08d3u64;
    for round in 0..rounds {
        let mut value = b"text/plain; x=\"".to_vec();
        for _ in 0..round % 257 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let byte = if round % 3 == 0 {
                32 + (seed % 95) as u8
            } else {
                seed as u8
            };
            value.push(byte);
        }
        if round % 2 == 0 {
            value.extend_from_slice(b"\"; charset=utf-8");
        }
        if let Ok(content_type) = ContentType::parse(&value) {
            let parameters: Vec<_> = content_type.parameters().map(Result::unwrap).collect();
            assert!(parameters.len() <= value.len());
            for parameter in parameters {
                let decoded = decoded(parameter.value());
                assert!(parameter.value().eq_ignore_ascii_case(&decoded).unwrap());
                assert!(!decoded.contains(&b'\r'));
                assert!(!decoded.contains(&b'\n'));
            }
        }
        let _ = ContentDisposition::parse(&value);
    }
    let valid = b"(prefix) multipart/mixed; boundary=\"a\\\"b\"; name=x (tail)";
    for length in 0..=valid.len() {
        let _ = ContentType::parse(&valid[..length]);
        let _ = ContentDisposition::parse(&valid[..length]);
    }
}
