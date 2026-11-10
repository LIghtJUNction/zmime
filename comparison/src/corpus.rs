//! Synthetic, deterministic fixtures. No private mail or external corpus is used.

#[derive(Clone)]
pub struct Case {
    pub name: String,
    pub raw: Vec<u8>,
}

pub fn base64(input: &[u8], wrap: bool) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        if wrap && !output.is_empty() && output.len() % 78 == 76 {
            output.extend_from_slice(b"\r\n");
        }
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[usize::from(a >> 2)]);
        output.push(ALPHABET[usize::from((a & 3) << 4 | b >> 4)]);
        output.push(if chunk.len() > 1 {
            ALPHABET[usize::from((b & 15) << 2 | c >> 6)]
        } else {
            b'='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[usize::from(c & 63)]
        } else {
            b'='
        });
    }
    output
}

pub fn qp(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(input.len() * 3);
    for (index, byte) in input.iter().copied().enumerate() {
        if index != 0 && index % 24 == 0 {
            out.extend_from_slice(b"=\r\n");
        }
        out.extend_from_slice(&[
            b'=',
            HEX[usize::from(byte >> 4)],
            HEX[usize::from(byte & 15)],
        ]);
    }
    out
}

fn single(headers: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = headers.as_bytes().to_vec();
    raw.extend_from_slice(b"\r\n");
    raw.extend_from_slice(body);
    raw
}

fn multipart(boundary: &str, children: &[Vec<u8>]) -> Vec<u8> {
    let mut raw =
        format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\nPreamble\r\n")
            .into_bytes();
    for child in children {
        raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        raw.extend_from_slice(child);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(format!("--{boundary}--\r\nEpilogue\r\n").as_bytes());
    raw
}

pub fn payload(len: usize) -> Vec<u8> {
    let mut seed = 0x54e7_a361_d82c_906bu64;
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        })
        .collect()
}

pub fn valid_cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "tiny".into(),
            raw: b"Subject: Hi\r\n\r\nx".to_vec(),
        },
        Case {
            name: "ordinary".into(),
            raw: single(
                "From: Alice <alice@example.test>\r\nTo: Bob <bob@example.test>\r\nDate: Tue, 10 Nov 2026 12:00:00 +0000\r\nSubject: Project status\r\nMessage-ID: <status.42@example.test>\r\nX-Trace: alpha\r\n\tbeta\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n",
                "The parser preserves raw bytes.\r\n附件按需解码。\r\n"
                    .repeat(20)
                    .as_bytes(),
            ),
        },
    ];
    let large = single(
        "Content-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"payload.bin\"\r\nContent-Transfer-Encoding: base64\r\n",
        &base64(&payload(1_048_576), true),
    );
    cases.push(Case {
        name: "large_attachment".into(),
        raw: multipart(
            "large-attachment",
            &[single("Content-Type: text/plain\r\n", b"Report"), large],
        ),
    });
    let mut nested = single(
        "Content-Type: text/plain; charset=us-ascii\r\n",
        b"deepest\r\n",
    );
    for level in (0..8).rev() {
        nested = multipart(
            &format!("nested-{level}"),
            &[single("Content-Type: text/plain\r\n", b"sibling"), nested],
        );
    }
    cases.push(Case {
        name: "nested_multipart".into(),
        raw: nested,
    });
    let news_text =
        "<article><h2>System notes</h2><p>SIMD, MIME and zero-copy parsing.</p></article>\r\n"
            .repeat(256);
    cases.push(Case {
        name: "newsletter".into(),
        raw: multipart("news", &[
            single("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n", b"Read the systems newsletter.\r\n"),
            single("Content-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n", &qp(news_text.as_bytes())),
        ]),
    });
    let mut long = b"Subject: Long folded field\r\n".to_vec();
    for _ in 0..100 {
        long.extend_from_slice(b" ");
        long.extend_from_slice(&[b'a'; 700]);
        long.extend_from_slice(b"\r\n");
    }
    long.extend_from_slice(b"X-End: yes\r\n\r\nbody\r\n");
    cases.push(Case {
        name: "long_headers".into(),
        raw: long,
    });
    let many: Vec<_> = (0..128)
        .map(|index| single(
            &format!("Content-Type: application/octet-stream; name=\"file-{index}.bin\"\r\nContent-Disposition: attachment; filename=\"file-{index}.bin\"\r\nContent-Transfer-Encoding: base64\r\n"),
            &base64(&payload(192), true),
        ))
        .collect();
    cases.push(Case {
        name: "many_attachments".into(),
        raw: multipart("many", &many),
    });
    cases.push(Case {
        name: "nested_message".into(),
        raw: multipart("forward", &[single("Content-Type: message/rfc822\r\n", b"From: Ann <ann@example.test>\r\nSubject: Forwarded\r\nContent-Type: text/plain\r\n\r\nInner body\r\n")]),
    });
    let lf: Vec<_> = cases
        .iter()
        .map(|case| Case {
            name: format!("{}_lf", case.name),
            raw: case
                .raw
                .iter()
                .copied()
                .filter(|byte| *byte != b'\r')
                .collect(),
        })
        .collect();
    cases.extend(lf);
    cases
}

pub fn malformed_cases() -> Vec<Case> {
    [
        ("empty", &b""[..]),
        ("missing_colon", &b"Subject broken\r\n\r\nbody"[..]),
        ("orphan_fold", &b" orphan\r\n\r\nbody"[..]),
        ("no_separator", &b"Subject: truncated"[..]),
        ("truncated_multipart", &b"Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody"[..]),
        ("invalid_base64", &b"Content-Transfer-Encoding: base64\r\n\r\nAA@="[..]),
        ("truncated_base64", &b"Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8"[..]),
        ("invalid_qp", &b"Content-Transfer-Encoding: quoted-printable\r\n\r\n=G1"[..]),
        ("truncated_qp", &b"Content-Transfer-Encoding: quoted-printable\r\n\r\nabc="[..]),
    ].into_iter().map(|(name, raw)| Case { name: name.into(), raw: raw.to_vec() }).collect()
}
