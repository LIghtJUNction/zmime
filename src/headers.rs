//! RFC 5322 field framing with borrowed raw values and explicit unfolding.
use crate::{
    scan::{find_byte, find_header_control},
    Error, Limit, Limits,
};

/// A validated block of header fields and the remaining unparsed body.
#[derive(Clone, Copy, Debug)]
pub struct HeaderBlock<'a> {
    pub(crate) raw: &'a [u8],
    pub(crate) body: &'a [u8],
    pub(crate) count: usize,
    first_colon: usize,
    first_end: usize,
}

impl<'a> HeaderBlock<'a> {
    pub(crate) const EMPTY: Self = Self {
        raw: b"",
        body: b"",
        count: 0,
        first_colon: 0,
        first_end: 0,
    };

    /// Validate field framing, names, values, continuations, and resource limits.
    ///
    /// CRLF and LF are supported, including mixed input. Bare CR and control
    /// bytes other than HTAB are rejected. Header names retain original case.
    /// Empty input is accepted. Nonempty input needs a blank separator line.
    /// UTF-8/non-ASCII value bytes are retained without implicit conversion.
    pub fn parse(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        Self::parse_with(input, limits, |_, _| Ok(()))
    }

    pub(crate) fn parse_with(
        input: &'a [u8],
        limits: Limits,
        mut on_header: impl FnMut(Header<'a>, usize) -> Result<(), Error>,
    ) -> Result<Self, Error> {
        if input.len() > limits.max_message_len {
            return Err(Error::LimitExceeded {
                limit: Limit::MessageBytes,
                offset: limits.max_message_len,
            });
        }
        if input.is_empty() {
            return Ok(Self::EMPTY);
        }
        let mut pos = 0;
        let mut count = 0;
        let mut field_start = 0;
        let mut colon_pos = 0;
        let mut value_end = 0;
        let mut first_colon = 0;
        let mut first_end = 0;
        loop {
            if pos >= input.len() {
                return Err(Error::MissingHeaderSeparator);
            }
            // Search only within the configured header and physical-line
            // budgets, so even an unbroken attacker-controlled line is bounded.
            let search_len = input
                .len()
                .min(limits.max_header_len)
                .min(pos.saturating_add(limits.max_line_len).saturating_add(2));
            if search_len <= pos {
                return Err(Error::LimitExceeded {
                    limit: Limit::HeaderBytes,
                    offset: pos,
                });
            }
            // The first forbidden control is either the line ending or a real
            // error. Combining framing and validation avoids scanning every
            // long field twice. HTAB is admitted by the scan kernel.
            let end = find_header_control(&input[pos..search_len]).map(|n| pos + n);
            let content_end = match end {
                Some(n) => n,
                None if search_len < input.len() => {
                    return Err(Error::LimitExceeded {
                        limit: if search_len == limits.max_header_len {
                            Limit::HeaderBytes
                        } else {
                            Limit::LineBytes
                        },
                        offset: search_len,
                    })
                }
                None => return Err(Error::MissingHeaderSeparator),
            };
            let end = match input[content_end] {
                b'\n' => content_end,
                b'\r' if input.get(content_end + 1) == Some(&b'\n') => content_end + 1,
                b'\r' if content_end + 1 == input.len() => {
                    return Err(Error::MissingHeaderSeparator)
                }
                _ => {
                    return Err(Error::InvalidHeaderByte {
                        offset: content_end,
                    })
                }
            };
            if content_end - pos > limits.max_line_len {
                return Err(Error::LimitExceeded {
                    limit: Limit::LineBytes,
                    offset: pos,
                });
            }
            if end + 1 > limits.max_header_len {
                return Err(Error::LimitExceeded {
                    limit: Limit::HeaderBytes,
                    offset: end,
                });
            }
            if content_end == pos {
                if count != 0 {
                    if count == 1 {
                        first_colon = colon_pos;
                        first_end = pos;
                    }
                    on_header(
                        Header {
                            name: &input[field_start..colon_pos],
                            value: &input[colon_pos + 1..value_end],
                            raw: &input[field_start..pos],
                        },
                        colon_pos + 1,
                    )?;
                }
                return Ok(Self {
                    raw: &input[..pos],
                    body: &input[end + 1..],
                    count,
                    first_colon,
                    first_end,
                });
            }
            let line = &input[pos..content_end];
            if matches!(line[0], b' ' | b'\t') {
                if count == 0 {
                    return Err(Error::InvalidHeader { offset: pos });
                }
            } else {
                if count != 0 {
                    if count == 1 {
                        first_colon = colon_pos;
                        first_end = pos;
                    }
                    on_header(
                        Header {
                            name: &input[field_start..colon_pos],
                            value: &input[colon_pos + 1..value_end],
                            raw: &input[field_start..pos],
                        },
                        colon_pos + 1,
                    )?;
                }
                let colon = find_byte(line, b':').ok_or(Error::InvalidHeader { offset: pos })?;
                if colon == 0
                    || !line[..colon]
                        .iter()
                        .all(|b| matches!(b, 33..=57 | 59..=126))
                {
                    return Err(Error::InvalidHeader { offset: pos });
                }
                count += 1;
                if count > limits.max_headers {
                    return Err(Error::LimitExceeded {
                        limit: Limit::Headers,
                        offset: pos,
                    });
                }
                field_start = pos;
                colon_pos = pos + colon;
            }
            value_end = content_end;
            pos = end + 1;
        }
    }

    /// Header bytes including individual line endings, without the blank line.
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }
    /// Body bytes following the blank separator, with no decoding.
    pub const fn body(&self) -> &'a [u8] {
        self.body
    }
    /// Number of logical fields (continuation lines do not increase this).
    pub const fn len(&self) -> usize {
        self.count
    }
    /// Whether the block contains no fields.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// Iterate fields in wire order, retaining duplicates.
    pub fn headers(&self) -> Headers<'a> {
        Headers {
            raw: self.raw,
            pos: 0,
            first_colon: self.first_colon,
            first_end: self.first_end,
        }
    }
    /// First field with the given ASCII case-insensitive name.
    pub fn get(&self, name: &[u8]) -> Option<Header<'a>> {
        self.headers()
            .find(|header| header.name.eq_ignore_ascii_case(name))
    }
}

/// One logical header, potentially spanning several physical lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header<'a> {
    name: &'a [u8],
    value: &'a [u8],
    raw: &'a [u8],
}

impl<'a> Header<'a> {
    /// Original field name, without the colon.
    pub const fn name(&self) -> &'a [u8] {
        self.name
    }
    /// Exact bytes after the colon, excluding the final physical line ending.
    /// Leading whitespace and intermediate fold line endings are preserved.
    pub const fn raw_value(&self) -> &'a [u8] {
        self.value
    }
    /// Entire field, including its final line ending.
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }
    /// RFC 5322 unfolding removes fold CRLF/LF and preserves following WSP.
    /// Returns the number of bytes written. No charset conversion is performed.
    /// The output is unchanged if it is too small.
    pub fn unfolded(&self, output: &mut [u8]) -> Result<usize, Error> {
        let needed = self
            .value
            .iter()
            .filter(|&&b| b != b'\r' && b != b'\n')
            .count();
        if output.len() < needed {
            return Err(Error::OutputTooSmall { needed });
        }
        let mut n = 0;
        for &b in self.value {
            if b != b'\r' && b != b'\n' {
                output[n] = b;
                n += 1;
            }
        }
        Ok(n)
    }
}

/// Allocation-free iterator over a previously validated header block.
#[derive(Clone, Debug)]
pub struct Headers<'a> {
    raw: &'a [u8],
    pos: usize,
    first_colon: usize,
    first_end: usize,
}

impl<'a> Iterator for Headers<'a> {
    type Item = Header<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos == self.raw.len() {
            return None;
        }
        // The first logical field is already located while validating the
        // block. Retaining two offsets avoids rescanning a long folded Subject
        // on every iteration without an input-sized header index or allocator.
        if self.pos == 0 && self.first_end != 0 {
            self.pos = self.first_end;
            let end = self.first_end - 1;
            let value_end = if end > self.first_colon && self.raw[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            return Some(Header {
                name: &self.raw[..self.first_colon],
                value: &self.raw[self.first_colon + 1..value_end],
                raw: &self.raw[..self.first_end],
            });
        }
        let start = self.pos;
        let colon = start + find_byte(&self.raw[start..], b':')?;
        let mut end = colon + 1;
        loop {
            end += find_byte(&self.raw[end..], b'\n')?;
            let next = end + 1;
            if next < self.raw.len() && matches!(self.raw[next], b' ' | b'\t') {
                end = next;
                continue;
            }
            self.pos = next;
            let value_end = if end > colon && self.raw[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            return Some(Header {
                name: &self.raw[start..colon],
                value: &self.raw[colon + 1..value_end],
                raw: &self.raw[start..next],
            });
        }
    }
}
impl core::iter::FusedIterator for Headers<'_> {}
