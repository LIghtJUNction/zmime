//! Differential checks and fixtures, isolated from the published crate.

pub mod consume;
pub mod corpus;

use mail_parser::{Encoding, HeaderForm, MessageParser};
use zmime::{Limits, Part};

pub fn limits() -> Limits {
    Limits {
        max_message_len: 16 * 1024 * 1024,
        max_header_len: 128 * 1024,
        max_headers: 1024,
        max_line_len: 128 * 1024,
        max_parts: 512,
        max_depth: 32,
    }
}

/// Disable semantics outside zmime's declared subset. MIME semantics remain on
/// in both libraries. Configuring this table is outside every timed region.
pub fn baseline() -> MessageParser {
    let mut parser = MessageParser::new()
        .unknown_headers(HeaderForm::Raw)
        .max_depth(32)
        .max_parts(512)
        .max_encoded_nesting(0);
    for name in [
        "Subject",
        "Comments",
        "Content-Description",
        "Content-Location",
        "From",
        "To",
        "Cc",
        "Bcc",
        "Reply-To",
        "Sender",
        "Resent-To",
        "Resent-From",
        "Resent-Bcc",
        "Resent-Cc",
        "Resent-Sender",
        "List-Archive",
        "List-Help",
        "List-Id",
        "List-Owner",
        "List-Post",
        "List-Subscribe",
        "List-Unsubscribe",
        "Date",
        "Resent-Date",
        "Message-ID",
        "References",
        "In-Reply-To",
        "Return-Path",
        "Content-ID",
        "Resent-Message-ID",
        "Keywords",
        "Content-Language",
        "Received",
    ] {
        parser = parser.header(name, HeaderForm::Raw);
    }
    parser
}

#[derive(Debug, PartialEq, Eq)]
pub struct MimeProjection {
    pub top_level: Vec<u8>,
    pub subtype: Vec<u8>,
    pub parameters: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PartProjection {
    pub parent: Option<usize>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub mime: MimeProjection,
    pub disposition: Option<MimeProjection>,
    pub encoding: u8,
    pub body: Vec<u8>,
}

fn without_final_eol(value: &[u8]) -> &[u8] {
    value
        .strip_suffix(b"\r\n")
        .or_else(|| value.strip_suffix(b"\n"))
        .unwrap_or(value)
}

pub fn project_zmime(part: &Part<'_>) -> PartProjection {
    let mime = part.content_type();
    let decode_parameters = |parameters: zmime::Parameters<'_>| {
        parameters
            .map(|parameter| {
                let parameter = parameter.expect("valid MIME parameter");
                let mut decoded = vec![0; parameter.value().raw().len() * 4 + 16];
                let length = parameter
                    .value()
                    .decode(&mut decoded)
                    .expect("supported parameter");
                decoded.truncate(length);
                (parameter.name().to_ascii_lowercase(), decoded)
            })
            .collect()
    };
    let parameters = decode_parameters(mime.parameters());
    PartProjection {
        parent: part.parent(),
        headers: part
            .headers()
            .headers()
            .map(|header| {
                (
                    header.name().to_vec(),
                    without_final_eol(header.raw_value()).to_vec(),
                )
            })
            .collect(),
        mime: MimeProjection {
            top_level: mime.top_level().to_ascii_lowercase(),
            subtype: mime.subtype().to_ascii_lowercase(),
            parameters,
        },
        disposition: part.disposition().map(|disposition| MimeProjection {
            top_level: disposition.kind().to_ascii_lowercase(),
            subtype: Vec::new(),
            parameters: decode_parameters(disposition.parameters()),
        }),
        encoding: match part.transfer_encoding() {
            zmime::TransferEncoding::Base64 => 1,
            zmime::TransferEncoding::QuotedPrintable => 2,
            _ => 0,
        },
        body: part.raw_body().to_vec(),
    }
}

pub fn project_mail(message: &mail_parser::Message<'_>) -> Vec<PartProjection> {
    let mut nested_parents = vec![None; message.parts().len()];
    for part in message.parts() {
        if let Some(message) = part.nested() {
            nested_parents[message.root_part().id() as usize] = Some(part.id() as usize);
        }
    }
    message
        .parts()
        .map(|part| {
            let mime = part.content_type().map_or_else(
                || MimeProjection {
                    top_level: if part.is_message() {
                        b"message".to_vec()
                    } else {
                        b"text".to_vec()
                    },
                    subtype: if part.is_message() {
                        b"rfc822".to_vec()
                    } else {
                        b"plain".to_vec()
                    },
                    parameters: vec![],
                },
                |mime| MimeProjection {
                    top_level: mime.ctype().as_bytes().to_vec(),
                    subtype: mime.subtype().unwrap_or("").as_bytes().to_vec(),
                    parameters: mime
                        .attributes()
                        .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
                        .collect(),
                },
            );
            PartProjection {
                parent: part
                    .parent()
                    .map(|parent| parent.id() as usize)
                    .or(nested_parents[part.id() as usize]),
                headers: part
                    .headers()
                    .iter()
                    .map(|header| {
                        (
                            header.raw_name().as_bytes().to_vec(),
                            without_final_eol(header.raw_value()).to_vec(),
                        )
                    })
                    .collect(),
                mime,
                disposition: part
                    .content_disposition()
                    .map(|disposition| MimeProjection {
                        top_level: disposition.ctype().as_bytes().to_vec(),
                        subtype: Vec::new(),
                        parameters: disposition
                            .attributes()
                            .map(|(key, value)| {
                                (key.as_bytes().to_vec(), value.as_bytes().to_vec())
                            })
                            .collect(),
                    }),
                encoding: match part.encoding() {
                    Encoding::Base64 => 1,
                    Encoding::QuotedPrintable => 2,
                    Encoding::None => 0,
                },
                body: part.raw_body().to_vec(),
            }
        })
        .collect()
}

pub fn assert_equivalent(case: &corpus::Case, parser: &MessageParser) {
    let mut workspace = vec![Part::EMPTY; 512];
    let mut scratch = vec![0; limits().max_header_len];
    let message = zmime::Message::parse(&case.raw, &mut workspace, limits())
        .unwrap_or_else(|error| panic!("{}: zmime rejected valid fixture: {error:?}", case.name));
    let reference = parser.parse(&case.raw).expect("valid mail-parser fixture");
    let expected = project_mail(&reference);
    assert_eq!(
        message.parts().len(),
        expected.len(),
        "{}: part count",
        case.name
    );
    for (index, (part, expected)) in message.parts().iter().zip(expected).enumerate() {
        assert_part(&project_zmime(part), &expected, &case.name, index);
    }
    for (ours, theirs) in message.parts().iter().zip(reference.parts()) {
        assert_eq!(
            consume::zmime(ours, &mut scratch),
            consume::reference(theirs),
            "{}: timed consumer",
            case.name
        );
    }
    let root = Part::parse_headers(&case.raw, limits()).expect("header-only parse");
    let reference = parser
        .parse_headers(&case.raw)
        .expect("reference header-only");
    assert_part(
        &project_zmime(&root),
        &project_mail(&reference).remove(0),
        &format!("{} headers", case.name),
        0,
    );
    assert_eq!(
        consume::zmime(&root, &mut scratch),
        consume::reference(reference.root_part()),
        "{}: timed headers consumer",
        case.name
    );
}

fn assert_part(ours: &PartProjection, reference: &PartProjection, name: &str, index: usize) {
    assert_eq!(ours.parent, reference.parent, "{name}: part {index} parent");
    assert_eq!(
        ours.headers, reference.headers,
        "{name}: part {index} headers"
    );
    assert_eq!(ours.mime, reference.mime, "{name}: part {index} media type");
    assert_eq!(
        ours.disposition, reference.disposition,
        "{name}: part {index} disposition"
    );
    assert_eq!(
        ours.encoding, reference.encoding,
        "{name}: part {index} encoding"
    );
    assert_eq!(
        ours.body.len(),
        reference.body.len(),
        "{name}: part {index} body length"
    );
    assert!(
        ours.body == reference.body,
        "{name}: part {index} body contents differ"
    );
}
