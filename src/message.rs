//! Iterative single-forward-scan MIME structure construction.
use crate::{
    ContentDisposition, ContentType, DecodeError, Error, HeaderBlock, Limit, Limits,
    TransferEncoding, Unsupported,
    decode::decode_text,
    mime::{Cursor, rebase},
    scan::find_byte,
};

/// One MIME entity. All content slices borrow the input; metadata lives in
/// caller-provided storage. Containers and message/rfc822 wrappers are nodes too.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    raw: &'a [u8],
    headers: HeaderBlock<'a>,
    content_type: ContentType<'a>,
    disposition: Option<ContentDisposition<'a>>,
    encoding: TransferEncoding,
    parent: Option<usize>,
    depth: usize,
    start: usize,
    body_start: usize,
    boundary: [u8; 70],
    boundary_len: u8,
    preamble: &'a [u8],
    epilogue: &'a [u8],
    epilogue_start: Option<usize>,
    boundary_seen: bool,
}

impl<'a> Part<'a> {
    /// Empty reusable workspace element, suitable for `[Part::EMPTY; N]`.
    pub const EMPTY: Self = Self {
        raw: b"",
        headers: HeaderBlock::EMPTY,
        content_type: ContentType::TEXT_PLAIN,
        disposition: None,
        encoding: TransferEncoding::SevenBit,
        parent: None,
        depth: 0,
        start: 0,
        body_start: 0,
        boundary: [0; 70],
        boundary_len: 0,
        preamble: b"",
        epilogue: b"",
        epilogue_start: None,
        boundary_seen: false,
    };

    /// Parse root headers and MIME metadata, leaving the entire body opaque.
    /// This does not scan boundaries, descendants, or transfer-encoded content.
    pub fn parse_headers(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        Self::with_default(input, limits, ContentType::TEXT_PLAIN)
    }

    fn with_default(
        input: &'a [u8],
        limits: Limits,
        default: ContentType<'a>,
    ) -> Result<Self, Error> {
        let mut part = Self {
            raw: input,
            content_type: default,
            ..Self::EMPTY
        };
        let mut seen = 0u8;
        let headers = HeaderBlock::parse_with(input, limits, |header, value_offset| {
            let name = header.name();
            let bit = if name.eq_ignore_ascii_case(b"content-type") {
                1
            } else if name.eq_ignore_ascii_case(b"content-disposition") {
                2
            } else if name.eq_ignore_ascii_case(b"content-transfer-encoding") {
                4
            } else {
                return Ok(());
            };
            if seen & bit != 0 {
                return Err(Error::DuplicateMimeHeader);
            }
            seen |= bit;
            let value = header.raw_value();
            let result = match bit {
                1 => ContentType::parse(value).map(|value| part.content_type = value),
                2 => ContentDisposition::parse(value).map(|value| part.disposition = Some(value)),
                _ => {
                    let parse_encoding = || {
                        let mut c = Cursor::new(value);
                        let value = c.token()?;
                        c.cfws()?;
                        if c.pos != c.bytes.len() {
                            return Err(c.error());
                        }
                        Ok(TransferEncoding::from_bytes(value))
                    };
                    parse_encoding().map(|value| part.encoding = value)
                }
            };
            result.map_err(|error| rebase(error, value_offset))
        })?;
        part.headers = headers;
        part.body_start = input.len() - headers.body.len();
        Ok(part)
    }

    fn init_boundary(&mut self) -> Result<(), Error> {
        let mut found = false;
        for param in self.content_type.parameters() {
            let param = param?;
            if param.name().eq_ignore_ascii_case(b"boundary") {
                if found {
                    return Err(Error::InvalidBoundary);
                }
                found = true;
                let len = param
                    .value()
                    .decode(&mut self.boundary)
                    .map_err(|_| Error::InvalidBoundary)?;
                if len == 0
                    || self.boundary[len - 1] == b' '
                    || !self.boundary[..len].iter().all(|&b| {
                        b.is_ascii_alphanumeric()
                            || matches!(
                                b,
                                b'\''
                                    | b'('
                                    | b')'
                                    | b'+'
                                    | b'_'
                                    | b','
                                    | b'-'
                                    | b'.'
                                    | b'/'
                                    | b':'
                                    | b'='
                                    | b'?'
                                    | b' '
                            )
                    })
                {
                    return Err(Error::InvalidBoundary);
                }
                self.boundary_len = len as u8;
            }
        }
        if !found {
            return Err(Error::InvalidBoundary);
        }
        if !self.identity_encoding() {
            return Err(Error::Unsupported(Unsupported::EncodedMultipart));
        }
        Ok(())
    }

    fn identity_encoding(&self) -> bool {
        matches!(
            self.encoding,
            TransferEncoding::SevenBit | TransferEncoding::EightBit | TransferEncoding::Binary
        )
    }
    fn boundary_bytes(&self) -> &[u8] {
        &self.boundary[..self.boundary_len as usize]
    }

    /// Entire raw MIME entity, including its headers and separator.
    /// The CRLF/LF introducing the parent's delimiter is excluded.
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }
    /// Validated headers paired with this part's raw body.
    pub const fn headers(&self) -> HeaderBlock<'a> {
        self.headers
    }
    /// Complete raw body. Attachment bytes are never decoded during parsing.
    pub const fn raw_body(&self) -> &'a [u8] {
        self.headers.body
    }
    /// Media type, including RFC 2045 / multipart/digest defaults.
    pub const fn content_type(&self) -> ContentType<'a> {
        self.content_type
    }
    /// Content-Disposition if present.
    pub const fn disposition(&self) -> Option<ContentDisposition<'a>> {
        self.disposition
    }
    /// Transfer encoding; unknown encodings are retained and fail on decode.
    pub const fn transfer_encoding(&self) -> TransferEncoding {
        self.encoding
    }
    /// Parent's index in [`Message::parts`], or `None` for the root.
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }
    /// Depth in the MIME tree, with root depth zero.
    pub const fn depth(&self) -> usize {
        self.depth
    }
    /// Multipart preamble without the line ending introducing its first delimiter.
    pub const fn preamble(&self) -> &'a [u8] {
        self.preamble
    }
    /// Multipart epilogue after its closing delimiter line.
    pub const fn epilogue(&self) -> &'a [u8] {
        self.epilogue
    }
    /// Whether Content-Disposition explicitly says `attachment`.
    pub fn is_attachment(&self) -> bool {
        self.disposition.map(|d| d.is_attachment()).unwrap_or(false)
    }
    /// Decode only the transfer encoding into caller-owned output.
    pub fn decode_body(&self, output: &mut [u8]) -> Result<usize, DecodeError> {
        self.encoding.decode(self.raw_body(), output)
    }
    /// Decode a text body into UTF-8. UTF-8, US-ASCII and ISO-8859-1 are
    /// supported; any other charset returns [`DecodeError::UnsupportedCharset`].
    /// An absent charset means US-ASCII. Output may contain a prefix on error.
    pub fn text<'o>(&self, output: &'o mut [u8]) -> Result<&'o str, DecodeError> {
        if !self.content_type.top_level().eq_ignore_ascii_case(b"text") {
            return Err(DecodeError::UnsupportedEncoding);
        }
        let mut label = [0u8; 64];
        let mut charset_parameter = None;
        for parameter in self.content_type.parameters() {
            let parameter = parameter.map_err(|_| DecodeError::UnsupportedCharset)?;
            let name = parameter.name();
            // RFC 2231 extended/continued parameters override an ordinary
            // fallback. Treating charset*=... as an absent charset would
            // silently interpret unsupported text as US-ASCII instead.
            if name.len() > 7 && name[..7].eq_ignore_ascii_case(b"charset") && name[7] == b'*' {
                return Err(DecodeError::UnsupportedCharset);
            }
            if charset_parameter.is_none() && name.eq_ignore_ascii_case(b"charset") {
                charset_parameter = Some(parameter);
            }
        }
        let charset = match charset_parameter {
            Some(p) => {
                let n = p
                    .value()
                    .decode(&mut label)
                    .map_err(|_| DecodeError::UnsupportedCharset)?;
                &label[..n]
            }
            None => b"us-ascii",
        };
        let n = decode_text(self.encoding, self.raw_body(), charset, output)?;
        core::str::from_utf8(&output[..n]).map_err(|_| DecodeError::InvalidUtf8)
    }
}
impl Default for Part<'_> {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// A validated MIME tree in caller-owned part storage.
///
/// Parsing is iterative. No stack frame or heap allocation is created per
/// input byte, header, or MIME level. MIME delimiters are discovered by one
/// forward body scan; at each candidate only currently open ancestors are
/// considered. The worst case is O(input length × configured depth).
///
/// On an error the workspace may contain a partial tree. A [`Message`] is
/// returned only when the entire structure and all header blocks validate.
#[derive(Debug)]
pub struct Message<'a, 'w> {
    raw: &'a [u8],
    parts: &'w [Part<'a>],
}

impl<'a, 'w> Message<'a, 'w> {
    /// Build a borrowed MIME tree. Every node, including the root and each
    /// multipart/message container, consumes one workspace element.
    ///
    /// Missing closing delimiters, ambiguous duplicate MIME headers/boundaries,
    /// and encoded multipart/message containers return explicit errors. An
    /// encoded message/rfc822 can instead be decoded with [`Part::parse_headers`]
    /// and [`Part::decode_body`], then parsed as a new input slice.
    pub fn parse(
        input: &'a [u8],
        workspace: &'w mut [Part<'a>],
        limits: Limits,
    ) -> Result<Self, Error> {
        if input.len() > limits.max_message_len {
            return Err(Error::LimitExceeded {
                limit: Limit::MessageBytes,
                offset: limits.max_message_len,
            });
        }
        let mut count = 0;
        let mut last = append_part(
            input,
            0,
            None,
            0,
            ContentType::TEXT_PLAIN,
            workspace,
            &mut count,
            limits,
        )?;
        let mut active = None;
        let mut pos;
        loop {
            let part = &workspace[last];
            if part.content_type.is(b"message", b"rfc822") {
                if !part.identity_encoding() {
                    return Err(Error::Unsupported(Unsupported::EncodedMessage));
                }
                last = append_part(
                    input,
                    part.body_start,
                    Some(last),
                    part.depth + 1,
                    ContentType::TEXT_PLAIN,
                    workspace,
                    &mut count,
                    limits,
                )?;
            } else {
                break;
            }
        }
        if workspace[last].content_type.is_multipart() {
            workspace[last].init_boundary()?;
            active = Some(last);
        }
        pos = workspace[last].body_start;
        while let Some(container) = active {
            let delimiter = find_delimiter(input, pos, container, workspace).ok_or(
                Error::UnclosedMultipart {
                    offset: input.len(),
                },
            )?;
            if delimiter.container != container {
                return Err(Error::UnclosedMultipart {
                    offset: delimiter.start,
                });
            }
            // Finalize the previous child branch before moving to its sibling.
            while last != container {
                let parent = workspace[last].parent;
                finalize(&mut workspace[last], input, delimiter.start, true);
                match parent {
                    Some(p) => last = p,
                    None => return Err(Error::InvalidBoundary),
                }
            }
            if !workspace[container].boundary_seen {
                if delimiter.closing {
                    return Err(Error::InvalidBoundary);
                }
                let body_start = workspace[container].body_start;
                let end = trim_delimiter_eol(input, body_start, delimiter.start);
                workspace[container].preamble = &input[body_start..end];
                workspace[container].boundary_seen = true;
            }
            pos = delimiter.after;
            if delimiter.closing {
                workspace[container].epilogue_start = Some(pos);
                active = enclosing_multipart(workspace[container].parent, workspace);
                continue;
            }
            let default = if workspace[container]
                .content_type
                .is(b"multipart", b"digest")
            {
                ContentType::MESSAGE_RFC822
            } else {
                ContentType::TEXT_PLAIN
            };
            last = append_part(
                input,
                pos,
                Some(container),
                workspace[container].depth + 1,
                default,
                workspace,
                &mut count,
                limits,
            )?;
            loop {
                let part = &workspace[last];
                if part.content_type.is(b"message", b"rfc822") {
                    if !part.identity_encoding() {
                        return Err(Error::Unsupported(Unsupported::EncodedMessage));
                    }
                    last = append_part(
                        input,
                        part.body_start,
                        Some(last),
                        part.depth + 1,
                        ContentType::TEXT_PLAIN,
                        workspace,
                        &mut count,
                        limits,
                    )?;
                } else {
                    break;
                }
            }
            if workspace[last].content_type.is_multipart() {
                workspace[last].init_boundary()?;
                let mut ancestor = Some(container);
                while let Some(n) = ancestor {
                    let ancestor_boundary = workspace[n].boundary_bytes();
                    let boundary = workspace[last].boundary_bytes();
                    // A trailing "--" turns an opening delimiter into a
                    // closing one. Either direction would make a child and
                    // ancestor delimiter indistinguishable on the wire.
                    if ancestor_boundary == boundary
                        || ancestor_boundary.strip_suffix(b"--") == Some(boundary)
                        || boundary.strip_suffix(b"--") == Some(ancestor_boundary)
                    {
                        return Err(Error::InvalidBoundary);
                    }
                    ancestor = enclosing_multipart(workspace[n].parent, workspace);
                }
                active = Some(last);
            }
            pos = workspace[last].body_start;
        }
        loop {
            finalize(&mut workspace[last], input, input.len(), false);
            match workspace[last].parent {
                Some(p) => last = p,
                None => break,
            }
        }
        Ok(Self {
            raw: input,
            parts: &workspace[..count],
        })
    }

    /// Complete original input, unchanged.
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }
    /// All nodes in depth-first wire order, including the root.
    pub const fn parts(&self) -> &'w [Part<'a>] {
        self.parts
    }
    /// The root MIME entity. A successful parse always contains a root.
    pub fn root(&self) -> &'w Part<'a> {
        &self.parts[0]
    }
    /// Explicit `Content-Disposition: attachment` nodes in wire order.
    pub fn attachments(&self) -> impl Iterator<Item = &'w Part<'a>> {
        self.parts.iter().filter(|p| p.is_attachment())
    }
    /// Direct children of a node, each paired with its index. Unknown parent
    /// indices yield an empty iterator.
    pub fn children(&self, parent: usize) -> impl Iterator<Item = (usize, &'w Part<'a>)> {
        self.parts
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.parent == Some(parent))
    }
}

#[allow(clippy::too_many_arguments)]
fn append_part<'a>(
    input: &'a [u8],
    start: usize,
    parent: Option<usize>,
    depth: usize,
    default: ContentType<'a>,
    parts: &mut [Part<'a>],
    count: &mut usize,
    limits: Limits,
) -> Result<usize, Error> {
    if depth > limits.max_depth {
        return Err(Error::LimitExceeded {
            limit: Limit::Depth,
            offset: start,
        });
    }
    if *count >= limits.max_parts {
        return Err(Error::LimitExceeded {
            limit: Limit::Parts,
            offset: start,
        });
    }
    if *count >= parts.len() {
        return Err(Error::WorkspaceTooSmall {
            needed: count.saturating_add(1),
        });
    }
    let mut part =
        Part::with_default(&input[start..], limits, default).map_err(|error| match error {
            Error::InvalidHeader { offset } => Error::InvalidHeader {
                offset: start + offset,
            },
            Error::InvalidHeaderByte { offset } => Error::InvalidHeaderByte {
                offset: start + offset,
            },
            Error::InvalidMime { offset } => Error::InvalidMime {
                offset: start + offset,
            },
            Error::LimitExceeded { limit, offset } => Error::LimitExceeded {
                limit,
                offset: start + offset,
            },
            other => other,
        })?;
    // An ancestor delimiter takes precedence even while a child header block
    // is being read (RFC 2046 §5.1.2). In particular, --a:b is otherwise a
    // syntactically valid field name/value and could swallow a truncated part.
    // Only scan the header region, never the remaining attachment body.
    if let Some(container) = enclosing_multipart(parent, parts) {
        if let Some(delimiter) =
            find_delimiter(&input[..start + part.body_start], start, container, parts)
        {
            return Err(Error::InvalidHeader {
                offset: delimiter.start,
            });
        }
    }
    part.parent = parent;
    part.depth = depth;
    part.start = start;
    part.body_start += start;
    let index = *count;
    parts[index] = part;
    *count += 1;
    Ok(index)
}

fn enclosing_multipart(mut parent: Option<usize>, parts: &[Part<'_>]) -> Option<usize> {
    while let Some(index) = parent {
        if parts[index].content_type.is_multipart() {
            return Some(index);
        }
        parent = parts[index].parent;
    }
    None
}
fn trim_delimiter_eol(input: &[u8], lower: usize, mut end: usize) -> usize {
    if end > lower && input[end - 1] == b'\n' {
        end -= 1;
        if end > lower && input[end - 1] == b'\r' {
            end -= 1;
        }
    }
    end
}
fn finalize<'a>(part: &mut Part<'a>, input: &'a [u8], end: usize, at_delimiter: bool) {
    let end = if at_delimiter {
        trim_delimiter_eol(input, part.body_start, end)
    } else {
        end
    };
    part.raw = &input[part.start..end];
    part.headers.body = &input[part.body_start..end];
    if let Some(start) = part.epilogue_start {
        part.epilogue = &input[start.min(end)..end];
    }
}
struct Delimiter {
    container: usize,
    start: usize,
    after: usize,
    closing: bool,
}
fn find_delimiter(
    input: &[u8],
    mut pos: usize,
    active: usize,
    parts: &[Part<'_>],
) -> Option<Delimiter> {
    while pos < input.len() {
        pos += find_byte(&input[pos..], b'-')?;
        if input.get(pos + 1) != Some(&b'-')
            || pos != parts[active].body_start && (pos == 0 || input[pos - 1] != b'\n')
        {
            pos += 1;
            continue;
        }
        let mut container = Some(active);
        while let Some(index) = container {
            let boundary = parts[index].boundary_bytes();
            let tail = &input[pos..];
            if tail.get(2..2 + boundary.len()) == Some(boundary) {
                let mut end = pos + 2 + boundary.len();
                let closing = input.get(end..end.saturating_add(2)) == Some(b"--");
                if closing {
                    end += 2;
                }
                while matches!(input.get(end), Some(b' ' | b'\t')) {
                    end += 1;
                }
                let after = if input.get(end..end.saturating_add(2)) == Some(b"\r\n") {
                    Some(end + 2)
                } else if input.get(end) == Some(&b'\n') {
                    Some(end + 1)
                } else if end == input.len() && closing {
                    Some(end)
                } else {
                    None
                };
                if let Some(after) = after {
                    return Some(Delimiter {
                        container: index,
                        start: pos,
                        after,
                        closing,
                    });
                }
            }
            container = enclosing_multipart(parts[index].parent, parts);
        }
        pos += 2;
    }
    None
}
