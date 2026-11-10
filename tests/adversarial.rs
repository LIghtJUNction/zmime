use zmime::{DecodeError, Error, Limit, Limits, Message, Part, Unsupported};

fn contains_slice(parent: &[u8], child: &[u8]) -> bool {
    child.is_empty() || parent.windows(child.len()).any(|window| window == child)
}

fn assert_tree(input: &[u8], message: &Message<'_, '_>) {
    assert_eq!(message.raw(), input);
    assert!(!message.parts().is_empty());
    assert_eq!(message.root().parent(), None);
    assert_eq!(message.root().depth(), 0);
    for (index, part) in message.parts().iter().enumerate() {
        assert!(contains_slice(input, part.raw()));
        assert!(part.raw().starts_with(part.headers().raw()));
        assert!(part.raw().ends_with(part.raw_body()));
        assert_eq!(part.headers().body(), part.raw_body());
        assert_eq!(part.headers().headers().count(), part.headers().len());
        if let Some(parent) = part.parent() {
            assert!(parent < index);
            assert_eq!(part.depth(), message.parts()[parent].depth() + 1);
            assert!(contains_slice(
                message.parts()[parent].raw_body(),
                part.raw()
            ));
        } else {
            assert_eq!(index, 0);
        }
    }
}

#[test]
fn ancestor_delimiter_must_not_be_swallowed_as_a_header() {
    let malformed = b"Content-Type: multipart/mixed; boundary=\"a:b\"\n\n--a:b\nX: one\n--a:b\nY: two\n\nbody\n--a:b--\n";
    let mut parts = [Part::EMPTY; 8];
    assert!(Message::parse(malformed, &mut parts, Limits::DEFAULT).is_err());

    let nested = b"Content-Type: multipart/mixed; boundary=\"a:b\"\n\n--a:b\nContent-Type: multipart/mixed; boundary=x\n\n--x\nX: one\n--a:b\nY: two\n\nbody\n--x--\n--a:b--\n";
    assert!(Message::parse(nested, &mut parts, Limits::DEFAULT).is_err());
}

#[test]
fn child_header_diagnostics_use_message_offsets() {
    let input =
        b"Content-Type: multipart/mixed; boundary=b\n\n--b\nBad Name: value\n\nbody\n--b--\n";
    let offset = input
        .windows(b"Bad Name".len())
        .position(|window| window == b"Bad Name")
        .unwrap();
    let mut parts = [Part::EMPTY; 4];
    assert_eq!(
        Message::parse(input, &mut parts, Limits::DEFAULT).unwrap_err(),
        Error::InvalidHeader { offset }
    );

    let input = b"Content-Type: message/rfc822\n\nX: bad\0value\n\nbody";
    let offset = input.iter().position(|&byte| byte == 0).unwrap();
    assert_eq!(
        Message::parse(input, &mut parts, Limits::DEFAULT).unwrap_err(),
        Error::InvalidHeaderByte { offset }
    );
}

#[test]
fn structured_header_diagnostics_use_the_public_input_offset() {
    for field in [
        "Content-Type: text/plain; name=",
        "Content-Disposition: attachment; name=",
        "Content-Transfer-Encoding: base64 garbage",
    ] {
        let leaf = format!("X-Prefix: yes\n{field}\n\nbody");
        let expected = if field.ends_with("garbage") {
            leaf.find("garbage").unwrap()
        } else {
            leaf.find("=\n").unwrap() + 1
        };
        assert_eq!(
            Part::parse_headers(leaf.as_bytes(), Limits::DEFAULT).unwrap_err(),
            Error::InvalidMime { offset: expected }
        );
        let prefix = "Content-Type: message/rfc822\n\n";
        let nested = format!("{prefix}{leaf}");
        let mut parts = [Part::EMPTY; 4];
        assert_eq!(
            Message::parse(nested.as_bytes(), &mut parts, Limits::DEFAULT).unwrap_err(),
            Error::InvalidMime {
                offset: prefix.len() + expected
            }
        );
    }
    let input = format!(
        "X: y\nContent-Type: text/plain {}x{}\n\n",
        "(".repeat(17),
        ")".repeat(17)
    );
    assert_eq!(
        Part::parse_headers(input.as_bytes(), Limits::DEFAULT).unwrap_err(),
        Error::LimitExceeded {
            limit: Limit::CommentDepth,
            offset: input.find('(').unwrap() + 16
        }
    );
}

#[test]
fn encoded_charset_parameters_must_not_fall_back_to_ascii() {
    for parameters in [
        "charset*=us-ascii''us-ascii",
        "charset*0=us-; charset*1=ascii",
        "charset*0*=us-ascii''us-; charset*1=ascii",
        "charset=us-ascii; charset*=us-ascii''big5",
        "charset*=us-ascii''big5; charset=us-ascii",
    ] {
        let input = format!("Content-Type: text/plain; {parameters}\n\nASCII payload");
        let part = Part::parse_headers(input.as_bytes(), Limits::DEFAULT).unwrap();
        let mut output = [0x55; 64];
        assert_eq!(part.text(&mut output), Err(DecodeError::UnsupportedCharset));
        assert_eq!(output, [0x55; 64]);
        assert_eq!(part.raw_body(), b"ASCII payload");
    }
}

#[test]
fn nested_opening_and_ancestor_closing_delimiters_cannot_be_identical() {
    for (outer, inner) in [("x", "x--"), ("x--", "x")] {
        let input = format!(
            "Content-Type: multipart/mixed; boundary={outer}\n\n--{outer}\nContent-Type: multipart/mixed; boundary={inner}\n\n--{inner}\n\nbody\n--{inner}--\n--{outer}--\n"
        );
        let mut parts = [Part::EMPTY; 8];
        assert_eq!(
            Message::parse(input.as_bytes(), &mut parts, Limits::DEFAULT).unwrap_err(),
            Error::InvalidBoundary
        );
    }
}

#[test]
fn outer_close_before_inner_close_is_an_error() {
    for newline in ["\n", "\r\n"] {
        let input = "Content-Type: multipart/mixed; boundary=outer\n\n--outer\nContent-Type: multipart/alternative; boundary=inner\n\n--inner\n\npayload\n--outer--\n".replace('\n', newline);
        let mut parts = [Part::EMPTY; 8];
        assert!(matches!(
            Message::parse(input.as_bytes(), &mut parts, Limits::DEFAULT),
            Err(Error::UnclosedMultipart { .. })
        ));
    }
}

#[test]
fn boundary_prefixes_do_not_confuse_nested_containers() {
    let input = b"Content-Type: multipart/mixed; boundary=a\n\n--a\nContent-Type: multipart/alternative; boundary=ab\n\n--ab\n\ninner\n--ab--\ninner epilogue\n--a\n\nouter sibling\n--a--\nouter epilogue";
    let mut parts = [Part::EMPTY; 8];
    let message = Message::parse(input, &mut parts, Limits::DEFAULT).unwrap();
    assert_tree(input, &message);
    assert_eq!(message.parts().len(), 4);
    assert_eq!(message.parts()[2].raw_body(), b"inner");
    assert_eq!(message.parts()[1].epilogue(), b"inner epilogue");
    assert_eq!(message.parts()[3].raw_body(), b"outer sibling");
    assert_eq!(message.root().epilogue(), b"outer epilogue");
    assert_eq!(
        message
            .children(0)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(message.children(usize::MAX).count(), 0);
}

#[test]
fn fake_boundary_lines_remain_part_of_the_body() {
    let input = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n--b \t\r\n\r\ntext--b\r\n--bX\r\n--b--garbage\r\n--b-\r\nlast\r\n--b--\t \r\n";
    let mut parts = [Part::EMPTY; 4];
    let message = Message::parse(input, &mut parts, Limits::DEFAULT).unwrap();
    assert_eq!(
        message.parts()[1].raw_body(),
        b"text--b\r\n--bX\r\n--b--garbage\r\n--b-\r\nlast"
    );
    assert_tree(input, &message);
}

#[test]
fn closing_delimiter_can_end_at_eof_and_introducing_eol_is_removed_once() {
    for newline in ["\n", "\r\n"] {
        let input =
            "Content-Type: multipart/mixed; boundary=b\n\npreamble\n--b\n\npayload\n\n--b--"
                .replace('\n', newline);
        let mut parts = [Part::EMPTY; 4];
        let message = Message::parse(input.as_bytes(), &mut parts, Limits::DEFAULT).unwrap();
        assert_eq!(message.root().preamble(), b"preamble");
        assert_eq!(
            message.parts()[1].raw_body(),
            format!("payload{newline}").as_bytes()
        );
        assert_tree(input.as_bytes(), &message);
    }
}

#[test]
fn boundary_grammar_and_length_are_enforced() {
    for boundary in ["a b", "a:b", "'()+_,-./:=?", &"a".repeat(70)] {
        let input = format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\n\n--{boundary}\n\nbody\n--{boundary}--\n");
        let mut parts = [Part::EMPTY; 4];
        let message = Message::parse(input.as_bytes(), &mut parts, Limits::DEFAULT).unwrap();
        assert_eq!(message.parts()[1].raw_body(), b"body");
    }
    for boundary in ["", " ", "trailing ", "a\tb", &"a".repeat(71)] {
        let input = format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\n\n");
        let mut parts = [Part::EMPTY; 4];
        assert!(matches!(
            Message::parse(input.as_bytes(), &mut parts, Limits::DEFAULT),
            Err(Error::InvalidBoundary)
        ));
    }
    for input in [
        b"Content-Type: multipart/mixed\n\n".as_slice(),
        b"Content-Type: multipart/mixed; boundary=x; BOUNDARY=x\n\n--x\n\nbody\n--x--\n",
        b"Content-Type: multipart/mixed; boundary=x\n\n--x\nContent-Type: multipart/mixed; boundary=x\n\n--x\n\nbody\n--x--\n--x--\n",
    ] {
        let mut parts = [Part::EMPTY; 8];
        assert!(matches!(Message::parse(input, &mut parts, Limits::DEFAULT), Err(Error::InvalidBoundary)));
    }
}

#[test]
fn tree_depth_and_workspace_budgets_stop_expansion() {
    let simple = b"\nbody";
    let mut one = [Part::EMPTY; 1];
    assert!(Message::parse(
        simple,
        &mut one,
        Limits {
            max_depth: 0,
            ..Limits::DEFAULT
        }
    )
    .is_ok());
    let nested = b"Content-Type: message/rfc822\n\n\nbody";
    let mut parts = [Part::EMPTY; 8];
    assert!(matches!(
        Message::parse(
            nested,
            &mut parts,
            Limits {
                max_depth: 0,
                ..Limits::DEFAULT
            }
        ),
        Err(Error::LimitExceeded {
            limit: Limit::Depth,
            ..
        })
    ));
    assert!(Message::parse(
        nested,
        &mut parts,
        Limits {
            max_depth: 1,
            ..Limits::DEFAULT
        }
    )
    .is_ok());
    assert!(matches!(
        Message::parse(nested, &mut one, Limits::DEFAULT),
        Err(Error::WorkspaceTooSmall { needed: 2 })
    ));
    assert!(matches!(
        Message::parse(simple, &mut [], Limits::DEFAULT),
        Err(Error::WorkspaceTooSmall { needed: 1 })
    ));
    assert!(matches!(
        Message::parse(
            simple,
            &mut parts,
            Limits {
                max_parts: 0,
                ..Limits::DEFAULT
            }
        ),
        Err(Error::LimitExceeded {
            limit: Limit::Parts,
            ..
        })
    ));
    let deep = "Content-Type: message/rfc822\n\n".repeat(64) + "\nbody";
    assert!(matches!(
        Message::parse(
            deep.as_bytes(),
            &mut parts,
            Limits {
                max_depth: 3,
                ..Limits::DEFAULT
            }
        ),
        Err(Error::LimitExceeded {
            limit: Limit::Depth,
            ..
        })
    ));
    let unlimited = Limits {
        max_depth: usize::MAX,
        max_parts: usize::MAX,
        ..Limits::DEFAULT
    };
    assert!(matches!(
        Message::parse(deep.as_bytes(), &mut one, unlimited),
        Err(Error::WorkspaceTooSmall { needed: 2 })
    ));
}

#[test]
fn encoded_containers_require_an_explicit_decode_step() {
    let rfc822 = b"Content-Type: message/rfc822\nContent-Transfer-Encoding: base64\n\nCgpib2R5";
    let multipart = b"Content-Type: multipart/mixed; boundary=x\nContent-Transfer-Encoding: quoted-printable\n\n--x=0A=0Abody=0A--x--";
    for (input, reason) in [
        (rfc822.as_slice(), Unsupported::EncodedMessage),
        (multipart.as_slice(), Unsupported::EncodedMultipart),
    ] {
        let mut parts = [Part::EMPTY; 4];
        assert!(
            matches!(Message::parse(input, &mut parts, Limits::DEFAULT), Err(Error::Unsupported(found)) if found == reason)
        );
        assert!(Part::parse_headers(input, Limits::DEFAULT).is_ok());
    }
}

#[test]
fn unsupported_charsets_and_broken_attachments_remain_lazy() {
    let input = b"Content-Type: multipart/mixed; boundary=x\n\n--x\nContent-Type: text/plain; charset=big5\n\nASCII bytes\n--x\nContent-Disposition: attachment; filename=bad.bin\nContent-Transfer-Encoding: base64\n\nnot-base64!!\n--x--\n";
    let mut parts = [Part::EMPTY; 8];
    let message = Message::parse(input, &mut parts, Limits::DEFAULT).unwrap();
    let mut output = [0u8; 128];
    assert_eq!(
        message.parts()[1].text(&mut output),
        Err(DecodeError::UnsupportedCharset)
    );
    let attachment = message.attachments().next().unwrap();
    assert_eq!(attachment.raw_body(), b"not-base64!!");
    assert!(matches!(
        attachment.decode_body(&mut output),
        Err(DecodeError::InvalidBase64 { .. })
    ));
}

#[test]
fn multipart_digest_children_default_to_embedded_messages() {
    let input = b"Content-Type: multipart/digest; boundary=x\n\n--x\n\nFrom: sender@example.invalid\nSubject: nested\n\nbody\n--x--\n";
    let mut parts = [Part::EMPTY; 8];
    let message = Message::parse(input, &mut parts, Limits::DEFAULT).unwrap();
    assert_eq!(message.parts().len(), 3);
    assert!(message.parts()[1].content_type().is(b"message", b"rfc822"));
    assert_eq!(
        message.parts()[2]
            .headers()
            .get(b"Subject")
            .unwrap()
            .raw_value(),
        b" nested"
    );
    assert_eq!(message.parts()[2].raw_body(), b"body");
    assert_tree(input, &message);
}

#[test]
fn every_nested_truncation_is_bounded() {
    let input = b"Content-Type: multipart/mixed; boundary=o\r\n\r\npre\r\n--o\r\nContent-Type: message/rfc822\r\n\r\nContent-Type: multipart/alternative; boundary=i\r\n\r\n--i\r\nContent-Type: text/plain\r\n\r\nbody\r\n--i--\r\ninner epilogue\r\n--o--\r\nouter epilogue";
    let step = if cfg!(miri) { 11 } else { 1 };
    for length in (0..=input.len()).step_by(step) {
        let mut parts = [Part::EMPTY; 8];
        if let Ok(message) = Message::parse(&input[..length], &mut parts, Limits::DEFAULT) {
            assert_tree(&input[..length], &message);
        }
    }
    let mut parts = [Part::EMPTY; 8];
    assert_tree(
        input,
        &Message::parse(input, &mut parts, Limits::DEFAULT).unwrap(),
    );
}

#[test]
fn deterministic_mutations_of_nested_mail_do_not_panic() {
    let template = b"Content-Type: multipart/mixed; boundary=\"o:x\"\n\n--o:x\nContent-Type: multipart/alternative; boundary=i\n\n--i\n\nbody\n--i--\n--o:x--\n";
    let rounds = if cfg!(miri) { 24 } else { 3000 };
    let mut seed = 0x1319_8a2e_0370_7344u64;
    for round in 0..rounds {
        let mut input = template.to_vec();
        for _ in 0..1 + round % 8 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let index = seed as usize % input.len();
            input[index] = (seed >> 32) as u8;
        }
        let mut parts = [Part::EMPTY; 8];
        if let Ok(message) = Message::parse(&input, &mut parts, Limits::DEFAULT) {
            assert_tree(&input, &message);
        }
    }
}
