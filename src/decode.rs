//! Transfer decoding into caller-owned memory.
//!
//! These decoders are strict about malformed escapes, Base64 padding, and
//! unsupported character sets. They do not allocate. On an error, the output
//! may contain a decoded prefix; callers must only use it after success.
//! [`DecodeError::OutputTooSmall`] reports the complete required length for an
//! otherwise valid input. No terminator is written.

use core::fmt;

#[path = "decode_simd.rs"]
mod simd;

/// An error while decoding a transfer encoding or an RFC 2047 encoded word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The destination cannot hold the decoded value.
    OutputTooSmall {
        /// Complete decoded length, in bytes.
        needed: usize,
    },
    /// Invalid alphabet, padding, unused padding bits, or a truncated quartet.
    InvalidBase64 {
        /// Byte offset in the input; the input length denotes a missing byte.
        offset: usize,
    },
    /// Invalid escape, line ending, or unencoded non-printable byte.
    InvalidQuotedPrintable {
        /// Byte offset in the input.
        offset: usize,
    },
    /// Malformed RFC 2047 syntax, including a word longer than 75 bytes.
    InvalidEncodedWord {
        /// Byte offset in the input.
        offset: usize,
    },
    /// A `7bit` body contains a byte with its high bit set.
    InvalidSevenBit {
        /// Byte offset in the input.
        offset: usize,
    },
    /// A US-ASCII encoded word contains a non-ASCII decoded byte.
    InvalidAscii,
    /// The encoded word names a character set this crate does not convert.
    UnsupportedCharset,
    /// A UTF-8 encoded word or literal text contains invalid UTF-8.
    InvalidUtf8,
    /// The Content-Transfer-Encoding value is not supported.
    UnsupportedEncoding,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputTooSmall { needed } => write!(f, "output needs {needed} bytes"),
            Self::InvalidBase64 { offset } => write!(f, "invalid Base64 at byte {offset}"),
            Self::InvalidQuotedPrintable { offset } => {
                write!(f, "invalid quoted-printable at byte {offset}")
            }
            Self::InvalidEncodedWord { offset } => {
                write!(f, "invalid encoded word at byte {offset}")
            }
            Self::InvalidSevenBit { offset } => write!(f, "non-ASCII 7bit body at byte {offset}"),
            Self::InvalidAscii => f.write_str("non-ASCII byte in a US-ASCII encoded word"),
            Self::UnsupportedCharset => f.write_str("unsupported character set"),
            Self::InvalidUtf8 => f.write_str("invalid UTF-8"),
            Self::UnsupportedEncoding => f.write_str("unsupported transfer encoding"),
        }
    }
}

/// MIME Content-Transfer-Encoding, without allocating a normalized name.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransferEncoding {
    /// The MIME default; decoded bytes must be ASCII.
    #[default]
    SevenBit,
    /// Unencoded bytes, with no character set conversion.
    EightBit,
    /// Unencoded arbitrary bytes.
    Binary,
    /// RFC 2045 Base64.
    Base64,
    /// RFC 2045 quoted-printable.
    QuotedPrintable,
    /// An unrecognized transfer encoding.
    Unknown,
}

impl TransferEncoding {
    /// Recognize a case-insensitive encoding name after trimming outer MIME
    /// whitespace. Missing fields should use [`Self::default`]; an empty
    /// explicitly supplied value is [`Self::Unknown`].
    #[must_use]
    pub fn from_bytes(mut value: &[u8]) -> Self {
        while value.first().is_some_and(|b| mime_space(*b)) {
            value = &value[1..];
        }
        while value.last().is_some_and(|b| mime_space(*b)) {
            value = &value[..value.len() - 1];
        }
        if value.eq_ignore_ascii_case(b"7bit") {
            Self::SevenBit
        } else if value.eq_ignore_ascii_case(b"8bit") {
            Self::EightBit
        } else if value.eq_ignore_ascii_case(b"binary") {
            Self::Binary
        } else if value.eq_ignore_ascii_case(b"base64") {
            Self::Base64
        } else if value.eq_ignore_ascii_case(b"quoted-printable") {
            Self::QuotedPrintable
        } else {
            Self::Unknown
        }
    }

    /// Decode this encoding into `output`, without character set conversion.
    pub fn decode(self, input: &[u8], output: &mut [u8]) -> Result<usize, DecodeError> {
        decode_transfer(self, input, output)
    }
}

/// Decode a MIME body into caller-provided memory.
///
/// `7bit` validates ASCII bytes. `8bit` and `binary` copy bytes unchanged.
/// Character set conversion is deliberately separate from transfer decoding.
pub fn decode_transfer(
    encoding: TransferEncoding,
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, DecodeError> {
    let mut out = Output::new(output);
    decode_transfer_to(encoding, input, &mut out)?;
    out.finish()
}

fn decode_transfer_to(
    encoding: TransferEncoding,
    input: &[u8],
    output: &mut impl Sink,
) -> Result<(), DecodeError> {
    match encoding {
        TransferEncoding::Base64 => decode_base64_to(input, output),
        TransferEncoding::QuotedPrintable => decode_quoted_printable_to(input, output),
        TransferEncoding::SevenBit => {
            if !input.is_ascii()
                && let Some(offset) = input.iter().position(|b| !b.is_ascii())
            {
                return Err(DecodeError::InvalidSevenBit { offset });
            }
            output.extend(input);
            Ok(())
        }
        TransferEncoding::EightBit | TransferEncoding::Binary => {
            output.extend(input);
            Ok(())
        }
        TransferEncoding::Unknown => Err(DecodeError::UnsupportedEncoding),
    }
}

trait Sink {
    fn extend(&mut self, bytes: &[u8]);

    fn base64_prefix(&mut self, _input: &[u8]) -> usize {
        0
    }
}

struct Output<'a> {
    bytes: &'a mut [u8],
    len: usize,
    backend: simd::Backend,
}

impl<'a> Output<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            len: 0,
            backend: simd::Backend::Unselected,
        }
    }

    fn finish(self) -> Result<usize, DecodeError> {
        if self.len <= self.bytes.len() {
            Ok(self.len)
        } else {
            Err(DecodeError::OutputTooSmall { needed: self.len })
        }
    }
}

impl Sink for Output<'_> {
    fn base64_prefix(&mut self, input: &[u8]) -> usize {
        if input.len() < 64 || self.bytes.len().saturating_sub(self.len) < 48 {
            return 0;
        }
        if matches!(self.backend, simd::Backend::Unselected) {
            self.backend = simd::Backend::detect();
        }
        let prefix = self
            .backend
            .base64_prefix(input, &mut self.bytes[self.len..]);
        self.len += prefix.written;
        prefix.read
    }

    #[inline(always)]
    fn extend(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let available = self.bytes.len().saturating_sub(self.len);
        if bytes.len() <= available {
            self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            return;
        }
        let copied = available.min(bytes.len());
        if copied != 0 {
            self.bytes[self.len..self.len + copied].copy_from_slice(&bytes[..copied]);
        }
        // Even Latin-1 conversion emits at most two bytes per input byte. A
        // Rust slice is limited to isize::MAX bytes, so a complete call cannot
        // overflow usize. Saturation also makes this helper robust in isolation.
        self.len = self.len.saturating_add(bytes.len());
    }
}

const fn base64_table() -> [u8; 256] {
    let mut table = [255; 256];
    let mut i = 0;
    while i < 26 {
        table[b'A' as usize + i] = i as u8;
        table[b'a' as usize + i] = (26 + i) as u8;
        i += 1;
    }
    i = 0;
    while i < 10 {
        table[b'0' as usize + i] = (52 + i) as u8;
        i += 1;
    }
    table[b'+' as usize] = 62;
    table[b'/' as usize] = 63;
    table
}

const BASE64: [u8; 256] = base64_table();

#[inline]
fn mime_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Decode RFC 2045 Base64, accepting CR, LF, space, and tab between symbols.
///
/// Other non-alphabet bytes, missing padding, data after padding, and nonzero
/// unused padding bits are rejected. Whitespace is accepted anywhere between
/// symbols, including within a quartet. An empty input decodes to zero bytes.
pub fn decode_base64(input: &[u8], output: &mut [u8]) -> Result<usize, DecodeError> {
    let mut out = Output::new(output);
    decode_base64_to(input, &mut out)?;
    out.finish()
}

fn decode_base64_to(input: &[u8], out: &mut impl Sink) -> Result<(), DecodeError> {
    let mut pos = 0;
    while pos < input.len() {
        pos += out.base64_prefix(&input[pos..]);
        // Decode four quartets together to amortize input/output bounds and
        // destination-capacity checks. A special symbol falls back to the
        // quartet path, so line folding and error offsets stay identical.
        while input.len() - pos >= 16 {
            let a = BASE64[input[pos] as usize];
            let b = BASE64[input[pos + 1] as usize];
            let c = BASE64[input[pos + 2] as usize];
            let d = BASE64[input[pos + 3] as usize];
            let e = BASE64[input[pos + 4] as usize];
            let f = BASE64[input[pos + 5] as usize];
            let g = BASE64[input[pos + 6] as usize];
            let h = BASE64[input[pos + 7] as usize];
            let i = BASE64[input[pos + 8] as usize];
            let j = BASE64[input[pos + 9] as usize];
            let k = BASE64[input[pos + 10] as usize];
            let l = BASE64[input[pos + 11] as usize];
            let m = BASE64[input[pos + 12] as usize];
            let n = BASE64[input[pos + 13] as usize];
            let o = BASE64[input[pos + 14] as usize];
            let p = BASE64[input[pos + 15] as usize];
            if a | b | c | d | e | f | g | h | i | j | k | l | m | n | o | p >= 64 {
                break;
            }
            out.extend(&[
                (a << 2) | (b >> 4),
                (b << 4) | (c >> 2),
                (c << 6) | d,
                (e << 2) | (f >> 4),
                (f << 4) | (g >> 2),
                (g << 6) | h,
                (i << 2) | (j >> 4),
                (j << 4) | (k >> 2),
                (k << 6) | l,
                (m << 2) | (n >> 4),
                (n << 4) | (o >> 2),
                (o << 6) | p,
            ]);
            pos += 16;
        }
        // Most MIME lines contain complete, unpadded quartets. Four table
        // lookups validate and decode a quartet without per-symbol branches.
        while input.len() - pos >= 4 {
            let a = BASE64[input[pos] as usize];
            let b = BASE64[input[pos + 1] as usize];
            let c = BASE64[input[pos + 2] as usize];
            let d = BASE64[input[pos + 3] as usize];
            if a | b | c | d >= 64 {
                break;
            }
            out.extend(&[(a << 2) | (b >> 4), (b << 4) | (c >> 2), (c << 6) | d]);
            pos += 4;
        }
        if pos == input.len() {
            break;
        }
        if mime_space(input[pos]) {
            while pos < input.len() && mime_space(input[pos]) {
                pos += 1;
            }
            continue;
        }
        let mut quartet = [0u8; 4];
        let mut offsets = [0usize; 4];
        let mut used = 0;
        while used < 4 && pos < input.len() {
            let byte = input[pos];
            let offset = pos;
            pos += 1;
            if mime_space(byte) {
                continue;
            }
            let value = if byte == b'=' {
                64
            } else {
                BASE64[byte as usize]
            };
            if value == 255 {
                return Err(DecodeError::InvalidBase64 { offset });
            }
            quartet[used] = value;
            offsets[used] = offset;
            used += 1;
        }
        if used == 0 {
            break;
        }
        if used != 4 {
            return Err(DecodeError::InvalidBase64 {
                offset: input.len(),
            });
        }
        let [a, b, c, d] = quartet;
        if a == 64 || b == 64 {
            return Err(DecodeError::InvalidBase64 {
                offset: offsets[usize::from(a != 64)],
            });
        }
        let padded = if c == 64 {
            if d != 64 || b & 15 != 0 {
                return Err(DecodeError::InvalidBase64 {
                    offset: offsets[if d != 64 { 3 } else { 1 }],
                });
            }
            out.extend(&[(a << 2) | (b >> 4)]);
            true
        } else if d == 64 {
            if c & 3 != 0 {
                return Err(DecodeError::InvalidBase64 { offset: offsets[2] });
            }
            out.extend(&[(a << 2) | (b >> 4), (b << 4) | (c >> 2)]);
            true
        } else {
            out.extend(&[(a << 2) | (b >> 4), (b << 4) | (c >> 2), (c << 6) | d]);
            false
        };
        if padded {
            if let Some(rest) = input[pos..].iter().position(|b| !mime_space(*b)) {
                return Err(DecodeError::InvalidBase64 { offset: pos + rest });
            }
            break;
        }
    }
    Ok(())
}

const fn hex_table() -> [u8; 256] {
    let mut table = [255; 256];
    let mut i = 0;
    while i < 10 {
        table[b'0' as usize + i] = i as u8;
        i += 1;
    }
    i = 0;
    while i < 6 {
        table[b'A' as usize + i] = 10 + i as u8;
        table[b'a' as usize + i] = 10 + i as u8;
        i += 1;
    }
    table
}

const HEX: [u8; 256] = hex_table();

const fn qp_hex_table() -> [u16; 512] {
    let mut table = [256; 512];
    let mut index = 0;
    while index < 256 {
        if HEX[index] < 16 {
            table[index] = (HEX[index] as u16) << 4;
            table[index + 256] = HEX[index] as u16;
        }
        index += 1;
    }
    table
}

// High and low nibbles share a table base. Bit eight marks an invalid digit,
// so one OR both combines the decoded byte and preserves validation failure.
const QP_HEX: [u16; 512] = qp_hex_table();

#[inline]
fn hex(byte: u8) -> Option<u8> {
    let value = HEX[byte as usize];
    (value < 16).then_some(value)
}

#[inline]
fn literal_run(input: &[u8], mut pos: usize) -> usize {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    pos += simd::literal_prefix(&input[pos..]);
    while input.len() - pos >= 8 {
        let block = &input[pos..pos + 8];
        let word = u64::from_le_bytes([
            block[0], block[1], block[2], block[3], block[4], block[5], block[6], block[7],
        ]);
        let equals = word ^ (LOW * b'=' as u64);
        let delete = word ^ (LOW * 127);
        // Whole-word subtraction can conservatively flag an extra byte after
        // a match through borrowing; only existence matters here. Every block
        // accepted by these tests consists entirely of printable ASCII other
        // than '='. Tabs and all exceptional blocks use the scalar check.
        if word & HIGH != 0
            || word.wrapping_sub(LOW * 32) & !word & HIGH != 0
            || equals.wrapping_sub(LOW) & !equals & HIGH != 0
            || delete.wrapping_sub(LOW) & !delete & HIGH != 0
        {
            break;
        }
        pos += 8;
    }
    while pos < input.len() && matches!(input[pos], b'\t' | b' '..=b'<' | b'>'..=b'~') {
        pos += 1;
    }
    pos
}

/// Decode RFC 2045 quoted-printable with CRLF or LF line endings.
///
/// Soft line breaks (`=\r\n` and `=\n`) are removed. Hard line endings are
/// preserved byte for byte. Unencoded spaces and tabs at a hard line ending or
/// at end of input are discarded as required by RFC 2045 section 6.7. Escaped
/// whitespace is preserved. Lowercase hexadecimal digits are accepted.
/// Malformed escapes, bare CR, and literal non-printable/non-ASCII bytes are
/// rejected. Physical line lengths are not restricted by this codec.
pub fn decode_quoted_printable(input: &[u8], output: &mut [u8]) -> Result<usize, DecodeError> {
    let mut out = Output::new(output);
    decode_quoted_printable_to(input, &mut out)?;
    out.finish()
}

fn decode_quoted_printable_to(input: &[u8], out: &mut impl Sink) -> Result<(), DecodeError> {
    let mut pos = 0;
    while pos < input.len() {
        if input[pos] == b'=' {
            if input.len() - pos >= 3 {
                let decoded =
                    QP_HEX[input[pos + 1] as usize] | QP_HEX[256 + input[pos + 2] as usize];
                if decoded < 256 {
                    out.extend(&[decoded as u8]);
                    pos += 3;
                    continue;
                }
                if input[pos + 1] == b'\r' && input[pos + 2] == b'\n' {
                    pos += 3;
                    continue;
                }
            }
            if input.get(pos + 1) == Some(&b'\n') {
                pos += 2;
                continue;
            }
            return Err(DecodeError::InvalidQuotedPrintable { offset: pos });
        }
        let start = pos;
        pos = literal_run(input, pos);
        let mut end = pos;
        if pos == input.len() || matches!(input[pos], b'\r' | b'\n') {
            while end > start && matches!(input[end - 1], b' ' | b'\t') {
                end -= 1;
            }
        }
        out.extend(&input[start..end]);
        if pos == input.len() {
            break;
        }
        match input[pos] {
            b'=' => {}
            b'\r' if input.get(pos + 1) == Some(&b'\n') => {
                out.extend(b"\r\n");
                pos += 2;
            }
            b'\n' => {
                out.extend(b"\n");
                pos += 1;
            }
            _ => return Err(DecodeError::InvalidQuotedPrintable { offset: pos }),
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Charset {
    Utf8,
    Ascii,
    Latin1,
}

impl Charset {
    fn parse(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.eq_ignore_ascii_case(b"utf-8") || bytes.eq_ignore_ascii_case(b"utf8") {
            Ok(Self::Utf8)
        } else if bytes.eq_ignore_ascii_case(b"us-ascii") || bytes.eq_ignore_ascii_case(b"ascii") {
            Ok(Self::Ascii)
        } else if bytes.eq_ignore_ascii_case(b"iso-8859-1")
            || bytes.eq_ignore_ascii_case(b"latin1")
            || bytes.eq_ignore_ascii_case(b"latin-1")
        {
            Ok(Self::Latin1)
        } else {
            Err(DecodeError::UnsupportedCharset)
        }
    }

    fn write(self, bytes: &[u8], out: &mut Output<'_>) -> Result<(), DecodeError> {
        match self {
            Self::Utf8 => {
                core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                out.extend(bytes);
            }
            Self::Ascii => {
                if !bytes.is_ascii() {
                    return Err(DecodeError::InvalidAscii);
                }
                out.extend(bytes);
            }
            Self::Latin1 => {
                for &byte in bytes {
                    if byte < 128 {
                        out.extend(&[byte]);
                    } else {
                        out.extend(&[0xc0 | (byte >> 6), 0x80 | (byte & 63)]);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Decode a MIME body's transfer encoding and convert its character set to
/// UTF-8 in one pass, using only caller-provided output memory.
///
/// UTF-8, US-ASCII, and ISO-8859-1 are supported, with the aliases documented
/// by [`decode_encoded_words`]. Unsupported labels return
/// [`DecodeError::UnsupportedCharset`] even when the body is empty. The label
/// must already be unquoted and unfolded. UTF-8 is validated across transfer
/// decoder blocks; ASCII rejects high-bit bytes; Latin-1 expands to UTF-8.
/// An undersized output reports the exact converted size for valid input.
pub fn decode_text(
    encoding: TransferEncoding,
    input: &[u8],
    charset: &[u8],
    output: &mut [u8],
) -> Result<usize, DecodeError> {
    let charset = Charset::parse(charset)?;
    let mut out = TextOutput {
        output: Output::new(output),
        charset,
        utf8: Utf8Validator::new(),
        invalid_ascii: false,
    };
    decode_transfer_to(encoding, input, &mut out)?;
    match charset {
        Charset::Utf8 if out.utf8.invalid || out.utf8.remaining != 0 => {
            return Err(DecodeError::InvalidUtf8);
        }
        Charset::Ascii if out.invalid_ascii => return Err(DecodeError::InvalidAscii),
        _ => {}
    }
    out.output.finish()
}

struct TextOutput<'a> {
    output: Output<'a>,
    charset: Charset,
    utf8: Utf8Validator,
    invalid_ascii: bool,
}

impl Sink for TextOutput<'_> {
    fn base64_prefix(&mut self, input: &[u8]) -> usize {
        // UTF-8 and ASCII preserve the decoded byte length, so they can use
        // the same bounded SIMD store as transfer-only decoding. Validate the
        // produced prefix before continuing; incomplete UTF-8 sequences stay
        // in the streaming validator across the scalar tail and MIME folds.
        if matches!(self.charset, Charset::Latin1) {
            return 0;
        }
        let start = self.output.len;
        let read = self.output.base64_prefix(input);
        if read != 0 {
            let bytes = &self.output.bytes[start..self.output.len];
            match self.charset {
                Charset::Utf8 => self.utf8.check(bytes),
                Charset::Ascii => self.invalid_ascii |= !bytes.is_ascii(),
                Charset::Latin1 => {}
            }
        }
        read
    }

    fn extend(&mut self, bytes: &[u8]) {
        match self.charset {
            Charset::Utf8 => {
                self.utf8.check(bytes);
                self.output.extend(bytes);
            }
            Charset::Ascii => {
                self.invalid_ascii |= !bytes.is_ascii();
                self.output.extend(bytes);
            }
            Charset::Latin1 => {
                if bytes.is_ascii() {
                    self.output.extend(bytes);
                } else {
                    for &byte in bytes {
                        if byte < 128 {
                            self.output.extend(&[byte]);
                        } else {
                            self.output
                                .extend(&[0xc0 | (byte >> 6), 0x80 | (byte & 63)]);
                        }
                    }
                }
            }
        }
    }
}

struct Utf8Validator {
    remaining: u8,
    low: u8,
    high: u8,
    invalid: bool,
}

impl Utf8Validator {
    fn new() -> Self {
        Self {
            remaining: 0,
            low: 0x80,
            high: 0xbf,
            invalid: false,
        }
    }

    fn check(&mut self, bytes: &[u8]) {
        if self.invalid || (self.remaining == 0 && bytes.is_ascii()) {
            return;
        }
        for &byte in bytes {
            if self.remaining != 0 {
                if byte < self.low || byte > self.high {
                    self.invalid = true;
                    return;
                }
                self.remaining -= 1;
                self.low = 0x80;
                self.high = 0xbf;
                continue;
            }
            match byte {
                0..=0x7f => {}
                0xc2..=0xdf => self.remaining = 1,
                0xe0 => {
                    self.remaining = 2;
                    self.low = 0xa0;
                }
                0xe1..=0xec | 0xee..=0xef => self.remaining = 2,
                0xed => {
                    self.remaining = 2;
                    self.high = 0x9f;
                }
                0xf0 => {
                    self.remaining = 3;
                    self.low = 0x90;
                }
                0xf1..=0xf3 => self.remaining = 3,
                0xf4 => {
                    self.remaining = 3;
                    self.high = 0x8f;
                }
                _ => {
                    self.invalid = true;
                    return;
                }
            }
        }
    }
}

struct Word<'a> {
    charset: Charset,
    base64: bool,
    text: &'a [u8],
    text_offset: usize,
    end: usize,
}

fn token_byte(byte: u8) -> bool {
    (33..=126).contains(&byte) && !b"()<>@,;:\\\"/[]?=".contains(&byte)
}

fn word(input: &[u8], start: usize) -> Result<Word<'_>, DecodeError> {
    let invalid = || DecodeError::InvalidEncodedWord { offset: start };
    let limit = start.saturating_add(75).min(input.len());
    let mut pos = start + 2;
    let charset_start = pos;
    while pos < limit && token_byte(input[pos]) {
        pos += 1;
    }
    if pos == charset_start || input.get(pos) != Some(&b'?') || pos >= limit {
        return Err(invalid());
    }
    let charset = Charset::parse(&input[charset_start..pos])?;
    let encoding = *input.get(pos + 1).ok_or_else(invalid)?;
    if !matches!(encoding, b'b' | b'B' | b'q' | b'Q') || input.get(pos + 2) != Some(&b'?') {
        return Err(invalid());
    }
    pos += 3;
    let text_offset = pos;
    while pos < limit && (33..=126).contains(&input[pos]) && input[pos] != b'?' {
        pos += 1;
    }
    if pos == text_offset
        || pos + 2 > limit
        || input.get(pos) != Some(&b'?')
        || input.get(pos + 1) != Some(&b'=')
    {
        return Err(invalid());
    }
    Ok(Word {
        charset,
        base64: matches!(encoding, b'b' | b'B'),
        text: &input[text_offset..pos],
        text_offset,
        end: pos + 2,
    })
}

fn decode_q_word(input: &[u8], output: &mut [u8], offset: usize) -> Result<usize, DecodeError> {
    let mut len = 0;
    let mut pos = 0;
    while pos < input.len() {
        let byte = match input[pos] {
            b'_' => {
                pos += 1;
                b' '
            }
            b'=' => {
                let high = input.get(pos + 1).and_then(|b| hex(*b));
                let low = input.get(pos + 2).and_then(|b| hex(*b));
                if let (Some(high), Some(low)) = (high, low) {
                    pos += 3;
                    high << 4 | low
                } else {
                    return Err(DecodeError::InvalidEncodedWord {
                        offset: offset + pos,
                    });
                }
            }
            byte => {
                pos += 1;
                byte
            }
        };
        // The caller supplies 75 bytes; a Q word cannot decode to more bytes
        // than its encoded-text, which is shorter than its 75-byte word.
        if let Some(dest) = output.get_mut(len) {
            *dest = byte;
        } else {
            return Err(DecodeError::OutputTooSmall { needed: len + 1 });
        }
        len += 1;
    }
    Ok(len)
}

fn skip_lwsp(input: &[u8], mut pos: usize) -> usize {
    while pos < input.len() {
        match input[pos] {
            b' ' | b'\t' => pos += 1,
            b'\r'
                if input.get(pos + 1) == Some(&b'\n')
                    && input
                        .get(pos + 2)
                        .is_some_and(|b| matches!(b, b' ' | b'\t')) =>
            {
                pos += 2;
            }
            b'\n'
                if input
                    .get(pos + 1)
                    .is_some_and(|b| matches!(b, b' ' | b'\t')) =>
            {
                pos += 1;
            }
            _ => break,
        }
    }
    pos
}

fn write_literal(input: &[u8], out: &mut Output<'_>) -> Result<(), DecodeError> {
    core::str::from_utf8(input).map_err(|_| DecodeError::InvalidUtf8)?;
    let mut start = 0;
    let mut pos = 0;
    while pos < input.len() {
        let fold = if input[pos] == b'\r' && input.get(pos + 1) == Some(&b'\n') {
            2
        } else if input[pos] == b'\n' {
            1
        } else {
            0
        };
        if fold != 0
            && input
                .get(pos + fold)
                .is_some_and(|b| matches!(b, b' ' | b'\t'))
        {
            out.extend(&input[start..pos]);
            pos += fold;
            start = pos;
        } else {
            pos += 1;
        }
    }
    out.extend(&input[start..]);
    Ok(())
}

/// Decode RFC 2047 encoded words in an unstructured header value to UTF-8.
///
/// Supports `B` and `Q` words in UTF-8, US-ASCII, and ISO-8859-1, including the
/// common `utf8`, `ascii`, `latin1`, and `latin-1` aliases. Other character sets
/// return [`DecodeError::UnsupportedCharset`]. Each word must fit the RFC 2047
/// 75-byte limit and contain complete characters.
///
/// Folding CRLF/LF before space or tab is removed. Whitespace between adjacent
/// encoded words is suppressed; other literal whitespace is preserved. Literal
/// text must be UTF-8. This function does not parse address/comment grammar;
/// callers select an appropriate unstructured value or phrase first.
pub fn decode_encoded_words(input: &[u8], output: &mut [u8]) -> Result<usize, DecodeError> {
    let mut out = Output::new(output);
    let mut decoded = [0u8; 75];
    let mut pos = 0;
    let mut previous_word = false;
    while pos < input.len() {
        if previous_word {
            let next = skip_lwsp(input, pos);
            if input[next..].starts_with(b"=?") {
                pos = next;
            }
        }
        if input[pos..].starts_with(b"=?") {
            let current = word(input, pos)?;
            let len = if current.base64 {
                decode_base64(current.text, &mut decoded).map_err(|err| match err {
                    DecodeError::InvalidBase64 { offset } => DecodeError::InvalidBase64 {
                        offset: current.text_offset + offset,
                    },
                    other => other,
                })?
            } else {
                decode_q_word(current.text, &mut decoded, current.text_offset)?
            };
            current.charset.write(&decoded[..len], &mut out)?;
            pos = current.end;
            previous_word = true;
        } else {
            let start = pos;
            pos += 1;
            while pos < input.len() && !input[pos..].starts_with(b"=?") {
                pos += 1;
            }
            write_literal(&input[start..pos], &mut out)?;
            previous_word = false;
        }
    }
    out.finish()
}
