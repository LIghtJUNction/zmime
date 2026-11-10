use zmime::decode::{
    decode_base64, decode_encoded_words, decode_quoted_printable, decode_text, decode_transfer,
    DecodeError, TransferEncoding,
};

fn decoded(
    decoder: fn(&[u8], &mut [u8]) -> Result<usize, DecodeError>,
    input: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let mut out = vec![0u8; input.len() * 2 + 16];
    let len = decoder(input, &mut out)?;
    out.truncate(len);
    Ok(out)
}

fn encode_base64(input: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        result.push(ALPHABET[(a >> 2) as usize]);
        result.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize]);
        result.push(if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize]
        } else {
            b'='
        });
        result.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize]
        } else {
            b'='
        });
    }
    result
}

fn encode_qp(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = Vec::new();
    for &byte in input {
        result.extend_from_slice(&[b'=', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]]);
    }
    result
}

#[test]
fn rfc4648_base64_vectors() {
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(
            decoded(decode_base64, encoded.as_bytes()).unwrap(),
            plain.as_bytes()
        );
    }
}

#[test]
fn base64_mime_whitespace_and_line_lengths() {
    assert_eq!(
        decoded(decode_base64, b" Z \tg\r\n= = \t\r\n").unwrap(),
        b"f"
    );
    assert_eq!(decoded(decode_base64, b"\r\n \t").unwrap(), b"");
    let input: Vec<_> = (0u8..=255)
        .cycle()
        .take(if cfg!(miri) { 511 } else { 8191 })
        .collect();
    let encoded = encode_base64(&input);
    for width in [1, 2, 3, 4, 5, 31, 75, 76, 77] {
        let mut wrapped = Vec::new();
        for line in encoded.chunks(width) {
            wrapped.extend_from_slice(line);
            wrapped.extend_from_slice(b"\r\n\t ");
        }
        assert_eq!(decoded(decode_base64, &wrapped).unwrap(), input);
    }
}

#[test]
fn base64_rejects_invalid_padding_and_alphabet() {
    for input in [
        &b"Z"[..],
        b"Zg",
        b"Zg=",
        b"Zg===",
        b"Zg==A",
        b"Zg==AAAA",
        b"=g==",
        b"Z===",
        b"====",
        b"Zg=A",
        b"Zh==",
        b"Zm9=",
        b"Zm9v-Yg=",
        b"Zm9v_Yg=",
        b"Zm9v\x0b",
        b"Zm9v\0",
        b"Zm9v\xff",
    ] {
        assert!(
            matches!(
                decoded(decode_base64, input),
                Err(DecodeError::InvalidBase64 { .. })
            ),
            "{input:?}"
        );
    }
    assert_eq!(
        decoded(decode_base64, b"a!aa"),
        Err(DecodeError::InvalidBase64 { offset: 1 })
    );
    assert_eq!(
        decoded(decode_base64, b"YQ== \nZ"),
        Err(DecodeError::InvalidBase64 { offset: 6 })
    );
}

#[test]
fn base64_all_byte_values_and_short_lengths() {
    let bytes: Vec<_> = (0u8..=255).collect();
    for len in 0..=256 {
        let encoded = encode_base64(&bytes[..len]);
        assert_eq!(decoded(decode_base64, &encoded).unwrap(), bytes[..len]);
    }
    for byte in 0u8..=255 {
        assert_eq!(
            decoded(decode_base64, &encode_base64(&[byte])).unwrap(),
            [byte]
        );
    }
}

#[test]
fn base64_checks_all_noncanonical_tail_bits() {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for (value, &symbol) in ALPHABET.iter().enumerate() {
        let one = [b'A', symbol, b'=', b'='];
        assert_eq!(decoded(decode_base64, &one).is_ok(), value & 15 == 0);
        let two = [b'A', b'A', symbol, b'='];
        assert_eq!(decoded(decode_base64, &two).is_ok(), value & 3 == 0);
    }
}

#[test]
fn rfc2045_quoted_printable() {
    let input = b"Now's the time =\r\nfor all folk to come=\r\n to the aid of their country.";
    let expected = b"Now's the time for all folk to come to the aid of their country.";
    assert_eq!(decoded(decode_quoted_printable, input).unwrap(), expected);
    assert_eq!(
        decoded(decode_quoted_printable, b"=00=3d=ff=09=20").unwrap(),
        b"\0=\xff\t "
    );
    assert_eq!(
        decoded(decode_quoted_printable, b"a=\nb\n\r\nc").unwrap(),
        b"ab\n\r\nc"
    );
    assert_eq!(
        decoded(decode_quoted_printable, b"literal_underscore").unwrap(),
        b"literal_underscore"
    );
}

#[test]
fn quoted_printable_trailing_whitespace() {
    assert_eq!(
        decoded(decode_quoted_printable, b"a \t\r\nb \t\nc \t").unwrap(),
        b"a\r\nb\nc"
    );
    assert_eq!(
        decoded(decode_quoted_printable, b"a=20\r\nb=09\nc=20").unwrap(),
        b"a \r\nb\t\nc "
    );
    assert_eq!(
        decoded(decode_quoted_printable, b"a \t=\r\nb").unwrap(),
        b"a \tb"
    );
    assert_eq!(
        decoded(decode_quoted_printable, b" \t\r\n\t\n  ").unwrap(),
        b"\r\n\n"
    );
}

#[test]
fn quoted_printable_all_bytes_and_invalid_input() {
    let bytes: Vec<_> = (0u8..=255).collect();
    assert_eq!(
        decoded(decode_quoted_printable, &encode_qp(&bytes)).unwrap(),
        bytes
    );
    for input in [
        &b"="[..],
        b"=A",
        b"=AX",
        b"=\r",
        b"= \r\n",
        b"a\rb",
        b"\0",
        b"\x7f",
        b"\xff",
        b"\x0b",
    ] {
        assert!(
            matches!(
                decoded(decode_quoted_printable, input),
                Err(DecodeError::InvalidQuotedPrintable { .. })
            ),
            "{input:?}"
        );
    }
}

#[test]
fn quoted_printable_dense_runs_preserve_boundaries_and_exact_capacity() {
    let bytes: Vec<_> = (0u8..=63).map(|byte| byte.wrapping_mul(137)).collect();
    for width in [3, 4, 5, 7, 8, 15, 16, 24, 64] {
        let mut encoded = Vec::new();
        for chunk in bytes.chunks(width) {
            encoded.extend_from_slice(&encode_qp(chunk));
            encoded.extend_from_slice(b"=\r\n");
        }
        encoded.extend_from_slice(b"literal=20 \t\r\n=09 \t");
        let mut expected = bytes.clone();
        expected.extend_from_slice(b"literal \r\n\t");
        for capacity in [0, 1, 3, 4, 5, expected.len() - 1, expected.len()] {
            let mut output = vec![0xa5; capacity + 2];
            let result = decode_quoted_printable(&encoded, &mut output[1..1 + capacity]);
            assert_eq!(
                result,
                if capacity < expected.len() {
                    Err(DecodeError::OutputTooSmall {
                        needed: expected.len(),
                    })
                } else {
                    Ok(expected.len())
                }
            );
            assert_eq!(&output[1..1 + capacity], &expected[..capacity]);
            assert_eq!(output[0], 0xa5);
            assert_eq!(output[capacity + 1], 0xa5);
        }
    }
}

#[test]
fn quoted_printable_dense_runs_keep_exact_error_offsets() {
    let mut input = b"=41".repeat(16);
    for position in 0..input.len() {
        let saved = input[position];
        for invalid in [0, b'!', b'G', 127, 128, 255] {
            if position % 3 == 0 && invalid.is_ascii_graphic() {
                continue;
            }
            input[position] = invalid;
            for capacity in [0, 3, 16] {
                let mut output = [0; 16];
                assert_eq!(
                    decode_quoted_printable(&input, &mut output[..capacity]),
                    Err(DecodeError::InvalidQuotedPrintable {
                        offset: position - position % 3,
                    }),
                    "byte {invalid}, position {position}, capacity {capacity}",
                );
            }
        }
        input[position] = saved;
    }
}

#[test]
fn transfer_encoding_recognition_and_binary() {
    for (name, expected) in [
        (&b"  7BiT\r\n"[..], TransferEncoding::SevenBit),
        (b"8bit", TransferEncoding::EightBit),
        (b"binary", TransferEncoding::Binary),
        (b"BaSe64", TransferEncoding::Base64),
        (b"quoted-PRINTABLE", TransferEncoding::QuotedPrintable),
        (b"x-custom", TransferEncoding::Unknown),
        (b"", TransferEncoding::Unknown),
    ] {
        assert_eq!(TransferEncoding::from_bytes(name), expected);
    }
    assert_eq!(TransferEncoding::default(), TransferEncoding::SevenBit);
    let mut output = [0u8; 8];
    assert_eq!(
        decode_transfer(TransferEncoding::SevenBit, b"x\xff", &mut output),
        Err(DecodeError::InvalidSevenBit { offset: 1 })
    );
    assert_eq!(
        decode_transfer(TransferEncoding::Unknown, b"x", &mut output),
        Err(DecodeError::UnsupportedEncoding)
    );
    assert_eq!(
        TransferEncoding::EightBit.decode(b"x\xff", &mut output),
        Ok(2)
    );
    assert_eq!(&output[..2], b"x\xff");
    assert_eq!(
        TransferEncoding::Binary.decode(b"\0\xff", &mut output),
        Ok(2)
    );
    assert_eq!(&output[..2], b"\0\xff");
}

#[test]
fn rfc2047_q_and_b_words() {
    for (input, expected) in [
        ("=?US-ASCII?Q?Keith_Moore?=", "Keith Moore"),
        ("=?ISO-8859-1?Q?Andr=E9?=", "André"),
        ("=?UTF-8?B?5L2g5aW9?=", "你好"),
        ("=?UTF-8?Q?=E4=BD=A0=E5=A5=BD?=", "你好"),
        ("Subject: =?utf-8?q?Hello_=E2=98=BA?=", "Subject: Hello ☺"),
        ("=?LATIN1?B?6f8=?=", "éÿ"),
        ("=?ascii?q?a=3Db=5Fc?=", "a=b_c"),
        ("plain UTF-8: 中文", "plain UTF-8: 中文"),
        ("", ""),
    ] {
        assert_eq!(
            decoded(decode_encoded_words, input.as_bytes()).unwrap(),
            expected.as_bytes()
        );
    }
}

#[test]
fn rfc2047_adjacent_words_and_folding() {
    for (input, expected) in [
        ("=?utf-8?Q?a?= =?utf-8?Q?b?=", "ab"),
        ("=?utf-8?Q?a?=\r\n\t =?utf-8?Q?b?=", "ab"),
        ("=?utf-8?Q?a?=\n\t=?utf-8?Q?b?=", "ab"),
        ("=?utf-8?Q?a?==?utf-8?Q?b?=", "ab"),
        ("=?utf-8?Q?a?= ordinary =?utf-8?Q?b?=", "a ordinary b"),
        ("=?utf-8?Q?a?= \r\n\t", "a \t"),
        ("before\r\n\t=?utf-8?Q?a?=\n after", "before\ta after"),
        ("=?utf-8?Q?a?=\n=?utf-8?Q?b?=", "a\nb"),
    ] {
        assert_eq!(
            decoded(decode_encoded_words, input.as_bytes()).unwrap(),
            expected.as_bytes()
        );
    }
}

#[test]
fn rfc2047_rejects_unknown_charsets_and_invalid_utf8() {
    assert_eq!(
        decoded(decode_encoded_words, b"=?gb18030?B?YWJj?="),
        Err(DecodeError::UnsupportedCharset)
    );
    assert_eq!(
        decoded(decode_encoded_words, b"=?windows-1252?Q?=80?="),
        Err(DecodeError::UnsupportedCharset)
    );
    assert_eq!(
        decoded(decode_encoded_words, b"=?us-ascii?Q?=E9?="),
        Err(DecodeError::InvalidAscii)
    );
    assert_eq!(
        decoded(decode_encoded_words, b"=?utf-8?Q?=E9?="),
        Err(DecodeError::InvalidUtf8)
    );
    assert_eq!(
        decoded(decode_encoded_words, b"=?utf-8?B?/w==?="),
        Err(DecodeError::InvalidUtf8)
    );
    assert_eq!(
        decoded(decode_encoded_words, b"raw\xff"),
        Err(DecodeError::InvalidUtf8)
    );
    // Each encoded word must contain complete characters, even if concatenating
    // two broken words would yield a valid UTF-8 sequence.
    assert_eq!(
        decoded(decode_encoded_words, b"=?utf-8?Q?=C3?= =?utf-8?Q?=A9?="),
        Err(DecodeError::InvalidUtf8)
    );
}

#[test]
fn rfc2047_rejects_malformed_words() {
    for input in [
        &b"=?"[..],
        b"=?utf-8",
        b"=?utf-8?",
        b"=?utf-8?Q",
        b"=?utf-8?Q?",
        b"=?utf-8?Q?abc",
        b"=?utf-8?Q?abc?",
        b"=?utf-8?X?abc?=",
        b"=?utf-8?QQ?abc?=",
        b"=?utf-8?Q??=",
        b"=?utf-8?Q?a b?=",
        b"=?utf-8?Q?a\tb?=",
        b"=?utf-8?Q?a\nb?=",
        b"=?utf-8?Q?=?=",
        b"=?utf-8?Q?=AQ?=",
        b"=?utf-8?Q?a?b?=",
    ] {
        assert!(
            matches!(
                decoded(decode_encoded_words, input),
                Err(DecodeError::InvalidEncodedWord { .. })
            ),
            "{input:?}"
        );
    }
    let valid = format!("=?utf-8?Q?{}?=", "a".repeat(63));
    assert_eq!(valid.len(), 75);
    assert_eq!(
        decoded(decode_encoded_words, valid.as_bytes())
            .unwrap()
            .len(),
        63
    );
    let invalid = format!("=?utf-8?Q?{}?=", "a".repeat(64));
    assert!(matches!(
        decoded(decode_encoded_words, invalid.as_bytes()),
        Err(DecodeError::InvalidEncodedWord { .. })
    ));
}

#[test]
fn output_capacity_is_exact_and_cannot_overwrite() {
    for (decoder, input) in [
        (
            decode_base64 as fn(&[u8], &mut [u8]) -> Result<usize, DecodeError>,
            &b"Zm9vYmFy"[..],
        ),
        (decode_quoted_printable, b"foo=00bar=20"),
        (decode_encoded_words, b"=?iso-8859-1?Q?caf=E9?="),
    ] {
        let expected = decoded(decoder, input).unwrap();
        for cap in 0..=expected.len() + 2 {
            let mut output = vec![0xa5; cap + 2];
            let result = decoder(input, &mut output[1..1 + cap]);
            if cap < expected.len() {
                assert_eq!(
                    result,
                    Err(DecodeError::OutputTooSmall {
                        needed: expected.len()
                    })
                );
            } else {
                assert_eq!(result, Ok(expected.len()));
            }
            let copied = cap.min(expected.len());
            assert_eq!(&output[1..1 + copied], &expected[..copied]);
            assert_eq!(output[0], 0xa5);
            assert!(output[1 + copied..].iter().all(|b| *b == 0xa5));
        }
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn deterministic_codec_round_trips() {
    let mut state = 0x3482_4a29_7253_c801;
    for _ in 0..if cfg!(miri) { 32 } else { 512 } {
        let len = (next_random(&mut state) % 1024) as usize;
        let input: Vec<_> = (0..len).map(|_| next_random(&mut state) as u8).collect();
        assert_eq!(
            decoded(decode_base64, &encode_base64(&input)).unwrap(),
            input
        );
        assert_eq!(
            decoded(decode_quoted_printable, &encode_qp(&input)).unwrap(),
            input
        );
    }
}

#[test]
fn arbitrary_and_truncated_inputs_remain_bounded() {
    let mut state = 0xf12a_8640_029d_331b;
    for _ in 0..if cfg!(miri) { 64 } else { 4096 } {
        let len = (next_random(&mut state) % 256) as usize;
        let input: Vec<_> = (0..len).map(|_| next_random(&mut state) as u8).collect();
        let capacity = (next_random(&mut state) % 512) as usize;
        for decoder in [decode_base64, decode_quoted_printable, decode_encoded_words] {
            let mut output = vec![0x8b; capacity + 2];
            if let Ok(written) = decoder(&input, &mut output[1..1 + capacity]) {
                assert!(written <= capacity);
            }
            assert_eq!(output[0], 0x8b);
            assert_eq!(output[capacity + 1], 0x8b);
        }
    }
    for input in [
        &b"=?UTF-8?B?5L2g5aW9?= and =?latin1?q?caf=E9?="[..],
        b"Zm9vYmFyYQ==",
        b"hard\r\nsoft=\r\n=E9",
    ] {
        for len in 0..input.len() {
            for decoder in [decode_base64, decode_quoted_printable, decode_encoded_words] {
                let _ = decoded(decoder, &input[..len]);
            }
        }
    }
}

#[test]
fn quoted_printable_literal_scan_matches_every_byte() {
    for byte in 0u8..=255 {
        for position in 0..32 {
            let mut input = [b'z'; 32];
            input[position] = byte;
            let value = decoded(decode_quoted_printable, &input);
            if matches!(byte, b'\t' | b'\n' | b' '..=b'<' | b'>'..=b'~') {
                let mut expected = input.to_vec();
                if position == 31 && matches!(byte, b' ' | b'\t') {
                    expected.pop();
                }
                assert_eq!(value.unwrap(), expected, "byte {byte}, position {position}");
            } else {
                assert_eq!(
                    value,
                    Err(DecodeError::InvalidQuotedPrintable { offset: position })
                );
            }
        }
    }
}

#[test]
fn body_text_latin1_conversion_and_exact_capacity() {
    let bytes: Vec<_> = (0u8..=255).collect();
    let expected: String = bytes.iter().copied().map(char::from).collect();
    for (encoding, input) in [
        (TransferEncoding::EightBit, bytes.clone()),
        (TransferEncoding::Base64, encode_base64(&bytes)),
        (TransferEncoding::QuotedPrintable, encode_qp(&bytes)),
    ] {
        for cap in [
            0,
            1,
            127,
            128,
            255,
            256,
            257,
            expected.len() - 1,
            expected.len(),
            512,
        ] {
            let mut output = vec![0xab; cap + 2];
            let result = decode_text(encoding, &input, b"iso-8859-1", &mut output[1..1 + cap]);
            if cap < expected.len() {
                assert_eq!(
                    result,
                    Err(DecodeError::OutputTooSmall {
                        needed: expected.len()
                    })
                );
            } else {
                assert_eq!(result, Ok(expected.len()));
            }
            let copied = cap.min(expected.len());
            assert_eq!(&output[1..1 + copied], &expected.as_bytes()[..copied]);
            assert_eq!(output[0], 0xab);
            assert!(output[1 + copied..].iter().all(|b| *b == 0xab));
        }
    }
}

#[test]
fn body_text_utf8_crosses_every_decoder_block_boundary() {
    let unicode: String = [
        0, 0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xe000, 0xffff, 0x10000, 0x10ffff,
    ]
    .into_iter()
    .map(|v| char::from_u32(v).unwrap())
    .collect();
    for prefix in 0..32 {
        let input = format!("{}{unicode}", "x".repeat(prefix));
        for (encoding, body) in [
            (TransferEncoding::Binary, input.as_bytes().to_vec()),
            (TransferEncoding::Base64, encode_base64(input.as_bytes())),
            (
                TransferEncoding::QuotedPrintable,
                encode_qp(input.as_bytes()),
            ),
        ] {
            let mut output = vec![0; input.len()];
            assert_eq!(
                decode_text(encoding, &body, b"UTF-8", &mut output),
                Ok(input.len())
            );
            assert_eq!(output, input.as_bytes());
            assert_eq!(
                decode_text(encoding, &body, b"utf-8", &mut []),
                Err(DecodeError::OutputTooSmall {
                    needed: input.len()
                })
            );
        }
    }
}

#[test]
fn body_text_rejects_invalid_characters_even_when_output_is_small() {
    for bytes in [
        &b"\x80"[..],
        b"\xc0\xaf",
        b"\xc1\xbf",
        b"\xc2",
        b"\xe0\x9f\xbf",
        b"\xed\xa0\x80",
        b"\xf0\x8f\xbf\xbf",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\xff",
        b"\xe2\x82",
        b"\xe2\x82x",
        b"\xc2\x7f",
    ] {
        let encoded = encode_base64(bytes);
        assert_eq!(
            decode_text(TransferEncoding::Base64, &encoded, b"utf-8", &mut []),
            Err(DecodeError::InvalidUtf8)
        );
    }
    assert_eq!(
        decode_text(TransferEncoding::Base64, b"/w==", b"us-ascii", &mut []),
        Err(DecodeError::InvalidAscii)
    );
    assert_eq!(
        decode_text(TransferEncoding::Binary, b"", b"gb18030", &mut []),
        Err(DecodeError::UnsupportedCharset)
    );
}

#[test]
fn body_text_vector_prefix_validates_bytes_before_capacity_errors() {
    let mut bytes = [b'a'; 257];
    let positions: Vec<_> = if cfg!(miri) {
        vec![0, 23, 24, 47, 48, 191, 192, 255, 256]
    } else {
        (0..bytes.len()).collect()
    };
    for position in positions {
        bytes[position] = 0x80;
        let encoded = encode_base64(&bytes);
        for capacity in [0, 48, 192, bytes.len()] {
            let mut output = [0; 257];
            for (charset, expected) in [
                (&b"utf-8"[..], DecodeError::InvalidUtf8),
                (&b"us-ascii"[..], DecodeError::InvalidAscii),
            ] {
                assert_eq!(
                    decode_text(
                        TransferEncoding::Base64,
                        &encoded,
                        charset,
                        &mut output[..capacity],
                    ),
                    Err(expected),
                    "position {position}, capacity {capacity}",
                );
            }
        }
        bytes[position] = b'a';
    }
}

#[test]
fn body_text_utf8_validator_agrees_with_core() {
    let mut state = 0x7f27_b011_35d4_8903;
    for _ in 0..if cfg!(miri) { 64 } else { 4096 } {
        let len = (next_random(&mut state) % 64) as usize;
        let input: Vec<_> = (0..len).map(|_| next_random(&mut state) as u8).collect();
        let mut output = vec![0; len];
        let actual = decode_text(
            TransferEncoding::Base64,
            &encode_base64(&input),
            b"utf-8",
            &mut output,
        );
        if core::str::from_utf8(&input).is_ok() {
            assert_eq!(actual, Ok(input.len()));
            assert_eq!(output, input);
        } else {
            assert_eq!(actual, Err(DecodeError::InvalidUtf8));
        }
    }
}

#[test]
fn base64_vector_blocks_respect_exact_slices_and_alignment() {
    let offsets = if cfg!(miri) { 4 } else { 32 };
    for len in [47, 48, 49, 63, 64, 65, 95, 96, 97, 128, 193, 257] {
        let bytes: Vec<_> = (0..len).map(|i| (i * 157 + 13) as u8).collect();
        let encoded = encode_base64(&bytes);
        for offset in 0..offsets {
            let mut source = vec![0; offset];
            source.extend_from_slice(&encoded);
            for capacity in [len - 1, len, len + 1] {
                let mut target = vec![0xac; offset + capacity + 1];
                let result =
                    decode_base64(&source[offset..], &mut target[offset..offset + capacity]);
                if capacity < len {
                    assert_eq!(result, Err(DecodeError::OutputTooSmall { needed: len }));
                } else {
                    assert_eq!(result, Ok(len));
                }
                let copied = capacity.min(len);
                assert_eq!(&target[offset..offset + copied], &bytes[..copied]);
                assert!(target[..offset].iter().all(|b| *b == 0xac));
                assert!(target[offset + copied..].iter().all(|b| *b == 0xac));
            }
        }
    }
}

#[test]
fn base64_vector_validation_checks_every_lane_and_byte() {
    let mut input = [b'A'; 128];
    let mut output = [0u8; 96];
    for position in 0..128 {
        for bad in [b'!', 0x80] {
            input[position] = bad;
            assert_eq!(
                decode_base64(&input, &mut output),
                Err(DecodeError::InvalidBase64 { offset: position })
            );
        }
        input[position] = b'A';
    }
    for byte in 0u8..=255 {
        input[17] = byte;
        let valid = byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/');
        assert_eq!(
            decode_base64(&input, &mut output).is_ok(),
            valid,
            "byte {byte}"
        );
    }
}

#[test]
fn parallel_decoding_keeps_caller_data_independent() {
    std::thread::scope(|scope| {
        for thread in 0..if cfg!(miri) { 2 } else { 8 } {
            scope.spawn(move || {
                let bytes: Vec<_> = (0..257).map(|i| (i * 17 + thread * 31) as u8).collect();
                let input = encode_base64(&bytes);
                let mut output = vec![0; bytes.len()];
                for _ in 0..if cfg!(miri) { 2 } else { 32 } {
                    assert_eq!(decode_base64(&input, &mut output), Ok(bytes.len()));
                    assert_eq!(output, bytes);
                }
            });
        }
    });
}
