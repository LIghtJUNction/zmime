//! Observable work shared by the timed parser paths.
//!
//! `zmime` leaves MIME quoted-string unescaping lazy. Reading only the type
//! token would omit work already performed by the reference parser. These
//! consumers therefore visit every supported metadata field and materialize
//! every parameter value before the timer stops.

use std::hint::black_box;

use mail_parser::{Encoding, MessagePart};
use zmime::{Part, TransferEncoding};

fn view(bytes: &[u8]) -> usize {
    black_box(bytes).len()
}

fn raw_value(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

pub fn zmime(part: &Part<'_>, scratch: &mut [u8]) -> usize {
    let mime = part.content_type();
    let mut checksum = view(part.raw_body())
        .wrapping_add(view(mime.top_level()))
        .wrapping_add(view(mime.subtype()));
    for parameter in mime.parameters() {
        let parameter = parameter.expect("validated MIME parameter");
        let length = parameter
            .value()
            .decode(scratch)
            .expect("supported parameter");
        checksum = checksum
            .wrapping_add(view(parameter.name()))
            .wrapping_add(view(&scratch[..length]));
    }
    if let Some(disposition) = part.disposition() {
        checksum = checksum.wrapping_add(view(disposition.kind()));
        for parameter in disposition.parameters() {
            let parameter = parameter.expect("validated disposition parameter");
            let length = parameter
                .value()
                .decode(scratch)
                .expect("supported parameter");
            checksum = checksum
                .wrapping_add(view(parameter.name()))
                .wrapping_add(view(&scratch[..length]));
        }
    }
    checksum = checksum.wrapping_add(match part.transfer_encoding() {
        TransferEncoding::Base64 => 1,
        TransferEncoding::QuotedPrintable => 2,
        _ => 0,
    });
    for header in part.headers().headers() {
        checksum = checksum
            .wrapping_add(view(header.name()))
            .wrapping_add(view(raw_value(header.raw_value())));
    }
    checksum
}

pub fn reference(part: MessagePart<'_>) -> usize {
    let mut checksum = view(part.raw_body());
    if let Some(mime) = part.content_type() {
        checksum = checksum
            .wrapping_add(view(mime.ctype().as_bytes()))
            .wrapping_add(view(mime.subtype().unwrap_or("").as_bytes()));
        for (name, value) in mime.attributes() {
            checksum = checksum
                .wrapping_add(view(name.as_bytes()))
                .wrapping_add(view(value.as_bytes()));
        }
    } else {
        // Apply the same RFC 2045 / multipart-digest defaults as the projection.
        checksum = checksum.wrapping_add(if part.is_message() { 13 } else { 9 });
    }
    if let Some(disposition) = part.content_disposition() {
        checksum = checksum.wrapping_add(view(disposition.ctype().as_bytes()));
        for (name, value) in disposition.attributes() {
            checksum = checksum
                .wrapping_add(view(name.as_bytes()))
                .wrapping_add(view(value.as_bytes()));
        }
    }
    checksum = checksum.wrapping_add(match part.encoding() {
        Encoding::Base64 => 1,
        Encoding::QuotedPrintable => 2,
        Encoding::None => 0,
    });
    for header in part.headers() {
        checksum = checksum
            .wrapping_add(view(header.raw_name().as_bytes()))
            .wrapping_add(view(raw_value(header.raw_value())));
    }
    checksum
}
