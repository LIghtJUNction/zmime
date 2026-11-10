//! Borrowed Content-Type and Content-Disposition fields.
use crate::{Error, Limit, Unsupported};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn error(&self) -> Error {
        Error::InvalidMime { offset: self.pos }
    }
    pub fn cfws(&mut self) -> Result<(), Error> {
        loop {
            match self.bytes.get(self.pos) {
                Some(b' ' | b'\t') => self.pos += 1,
                Some(b'\r' | b'\n') => self.fold()?,
                Some(b'(') => {
                    self.pos += 1;
                    let mut depth = 1usize;
                    while depth != 0 {
                        match self.bytes.get(self.pos).copied() {
                            Some(b'(') => {
                                depth += 1;
                                if depth > 16 {
                                    return Err(Error::LimitExceeded {
                                        limit: Limit::CommentDepth,
                                        offset: self.pos,
                                    });
                                }
                                self.pos += 1;
                            }
                            Some(b')') => {
                                depth -= 1;
                                self.pos += 1;
                            }
                            Some(b'\\') => {
                                self.pos += 1;
                                match self.bytes.get(self.pos) {
                                    Some(9 | 32..=126) => self.pos += 1,
                                    _ => return Err(self.error()),
                                }
                            }
                            Some(b'\r' | b'\n') => self.fold()?,
                            Some(9 | 32..=126) => self.pos += 1,
                            _ => return Err(self.error()),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }
    fn fold(&mut self) -> Result<(), Error> {
        if self.bytes[self.pos] == b'\r' {
            self.pos += 1;
            if self.bytes.get(self.pos) != Some(&b'\n') {
                return Err(self.error());
            }
        }
        self.pos += 1;
        if !matches!(self.bytes.get(self.pos), Some(b' ' | b'\t')) {
            return Err(self.error());
        }
        Ok(())
    }
    pub fn token(&mut self) -> Result<&'a [u8], Error> {
        self.cfws()?;
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .copied()
            .map(is_token)
            .unwrap_or(false)
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error());
        }
        Ok(&self.bytes[start..self.pos])
    }
    fn value(&mut self) -> Result<ParameterValue<'a>, Error> {
        self.cfws()?;
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Ok(ParameterValue {
                raw: self.token()?,
                quoted: false,
                extended: false,
                needs_unescape: false,
            });
        }
        self.pos += 1;
        let start = self.pos;
        let mut needs_unescape = false;
        loop {
            match self.bytes.get(self.pos) {
                Some(b'"') => {
                    let raw = &self.bytes[start..self.pos];
                    self.pos += 1;
                    return Ok(ParameterValue {
                        raw,
                        quoted: true,
                        extended: false,
                        needs_unescape,
                    });
                }
                Some(b'\\') => {
                    needs_unescape = true;
                    self.pos += 1;
                    match self.bytes.get(self.pos) {
                        Some(9 | 32..=126) => self.pos += 1,
                        _ => return Err(self.error()),
                    }
                }
                Some(b'\r' | b'\n') => {
                    needs_unescape = true;
                    self.fold()?;
                }
                Some(9 | 32..=126) => self.pos += 1,
                _ => return Err(self.error()),
            }
        }
    }
}
fn is_token(b: u8) -> bool {
    b > 32
        && b < 127
        && !matches!(
            b,
            b'(' | b')'
                | b'<'
                | b'>'
                | b'@'
                | b','
                | b';'
                | b':'
                | b'\\'
                | b'"'
                | b'/'
                | b'['
                | b']'
                | b'?'
                | b'='
        )
}

/// A syntactically validated media type; tokens retain their original case.
#[derive(Clone, Copy, Debug)]
pub struct ContentType<'a> {
    top: &'a [u8],
    sub: &'a [u8],
    params: &'a [u8],
}
impl<'a> ContentType<'a> {
    /// RFC 2045 default for an ordinary part without Content-Type.
    pub const TEXT_PLAIN: Self = Self {
        top: b"text",
        sub: b"plain",
        params: b"",
    };
    /// RFC 2046 default for a direct child of multipart/digest.
    pub const MESSAGE_RFC822: Self = Self {
        top: b"message",
        sub: b"rfc822",
        params: b"",
    };
    /// Parse the field value, including comments, folds, quoted strings, and parameters.
    pub fn parse(value: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(value);
        let top = c.token()?;
        c.cfws()?;
        if c.bytes.get(c.pos) != Some(&b'/') {
            return Err(c.error());
        }
        c.pos += 1;
        let sub = c.token()?;
        let params = &value[c.pos..];
        for p in Parameters::new(params) {
            p.map_err(|e| rebase(e, c.pos))?;
        }
        Ok(Self { top, sub, params })
    }
    /// Top-level type, for example `text` or `multipart`.
    pub const fn top_level(&self) -> &'a [u8] {
        self.top
    }
    /// Subtype, for example `plain` or `mixed`.
    pub const fn subtype(&self) -> &'a [u8] {
        self.sub
    }
    /// ASCII case-insensitive media type comparison.
    pub fn is(&self, top: &[u8], sub: &[u8]) -> bool {
        self.top.eq_ignore_ascii_case(top) && self.sub.eq_ignore_ascii_case(sub)
    }
    /// Whether the top-level type is multipart.
    pub fn is_multipart(&self) -> bool {
        self.top.eq_ignore_ascii_case(b"multipart")
    }
    /// Iterate every parameter in input order, retaining duplicate parameters.
    pub fn parameters(&self) -> Parameters<'a> {
        Parameters::new(self.params)
    }
    /// First parameter matching an ASCII case-insensitive name.
    pub fn parameter(&self, name: &[u8]) -> Result<Option<Parameter<'a>>, Error> {
        lookup(self.parameters(), name)
    }
}

/// A syntactically validated disposition and its borrowed parameters.
#[derive(Clone, Copy, Debug)]
pub struct ContentDisposition<'a> {
    kind: &'a [u8],
    params: &'a [u8],
}
impl<'a> ContentDisposition<'a> {
    /// Parse a Content-Disposition value with MIME parameter syntax.
    pub fn parse(value: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(value);
        let kind = c.token()?;
        let params = &value[c.pos..];
        for p in Parameters::new(params) {
            p.map_err(|e| rebase(e, c.pos))?;
        }
        Ok(Self { kind, params })
    }
    /// Disposition token, for example `inline` or `attachment`.
    pub const fn kind(&self) -> &'a [u8] {
        self.kind
    }
    /// Whether this disposition explicitly declares an attachment.
    pub fn is_attachment(&self) -> bool {
        self.kind.eq_ignore_ascii_case(b"attachment")
    }
    /// Parameters in input order.
    pub fn parameters(&self) -> Parameters<'a> {
        Parameters::new(self.params)
    }
    /// First parameter with the supplied ASCII case-insensitive name.
    pub fn parameter(&self, name: &[u8]) -> Result<Option<Parameter<'a>>, Error> {
        lookup(self.parameters(), name)
    }
}
fn lookup<'a>(params: Parameters<'a>, name: &[u8]) -> Result<Option<Parameter<'a>>, Error> {
    for p in params {
        let p = p?;
        if p.name.eq_ignore_ascii_case(name) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}
pub(crate) fn rebase(error: Error, base: usize) -> Error {
    match error {
        Error::InvalidMime { offset } => Error::InvalidMime {
            offset: base + offset,
        },
        Error::LimitExceeded { limit, offset } => Error::LimitExceeded {
            limit,
            offset: base + offset,
        },
        other => other,
    }
}

/// A borrowed MIME parameter. Extended names remain visible without conversion.
#[derive(Clone, Copy, Debug)]
pub struct Parameter<'a> {
    name: &'a [u8],
    value: ParameterValue<'a>,
}
impl<'a> Parameter<'a> {
    /// Raw attribute token.
    pub const fn name(&self) -> &'a [u8] {
        self.name
    }
    /// Value retaining quoted-pairs and folding until explicitly decoded.
    pub const fn value(&self) -> ParameterValue<'a> {
        self.value
    }
}

/// Parameter bytes with optional quoted-string unescaping.
#[derive(Clone, Copy, Debug)]
pub struct ParameterValue<'a> {
    raw: &'a [u8],
    quoted: bool,
    extended: bool,
    needs_unescape: bool,
}
impl<'a> ParameterValue<'a> {
    /// Original value bytes, without surrounding quotes.
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }
    /// Whether the value was enclosed in double quotes.
    pub const fn is_quoted(&self) -> bool {
        self.quoted
    }
    /// Whether its name requests RFC 2231 extended/continuation semantics.
    pub const fn is_extended(&self) -> bool {
        self.extended
    }
    /// Unescape quoted-pairs and unfold into a caller-owned buffer.
    /// RFC 2231 extended parameters explicitly return `Unsupported`.
    /// The output is unchanged when it is too short.
    pub fn decode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if self.extended {
            return Err(Error::Unsupported(Unsupported::ExtendedParameter));
        }
        // Validation already established whether a quoted value contains a
        // quoted-pair or fold. Ordinary tokens and unescaped quoted strings
        // therefore need neither a counting pass nor a byte-at-a-time copy.
        if !self.needs_unescape {
            let needed = self.raw.len();
            if out.len() < needed {
                return Err(Error::OutputTooSmall { needed });
            }
            out[..needed].copy_from_slice(self.raw);
            return Ok(needed);
        }
        let needed = self.bytes().count();
        if out.len() < needed {
            return Err(Error::OutputTooSmall { needed });
        }
        for (n, b) in self.bytes().enumerate() {
            out[n] = b;
        }
        Ok(needed)
    }
    /// Compare the unescaped value against ASCII without allocation.
    pub fn eq_ignore_ascii_case(&self, value: &[u8]) -> Result<bool, Error> {
        if self.extended {
            return Err(Error::Unsupported(Unsupported::ExtendedParameter));
        }
        if !self.needs_unescape {
            return Ok(self.raw.eq_ignore_ascii_case(value));
        }
        let mut bytes = self.bytes();
        for &b in value {
            if !bytes
                .next()
                .map(|a| a.eq_ignore_ascii_case(&b))
                .unwrap_or(false)
            {
                return Ok(false);
            }
        }
        Ok(bytes.next().is_none())
    }
    fn bytes(&self) -> ValueBytes<'a> {
        ValueBytes {
            raw: self.raw,
            pos: 0,
            quoted: self.quoted,
        }
    }
}
struct ValueBytes<'a> {
    raw: &'a [u8],
    pos: usize,
    quoted: bool,
}
impl Iterator for ValueBytes<'_> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        loop {
            let mut b = *self.raw.get(self.pos)?;
            self.pos += 1;
            if self.quoted && b == b'\\' {
                b = *self.raw.get(self.pos)?;
                self.pos += 1;
                return Some(b);
            }
            if b != b'\r' && b != b'\n' {
                return Some(b);
            }
        }
    }
}

/// MIME parameter iterator. It stops after its first error.
#[derive(Clone, Debug)]
pub struct Parameters<'a> {
    cursor: Cursor<'a>,
    done: bool,
}
impl<'a> Parameters<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            done: false,
        }
    }
}
impl<'a> Iterator for Parameters<'a> {
    type Item = Result<Parameter<'a>, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let result = (|| {
            self.cursor.cfws()?;
            if self.cursor.pos == self.cursor.bytes.len() {
                return Ok(None);
            }
            if self.cursor.bytes[self.cursor.pos] != b';' {
                return Err(self.cursor.error());
            }
            self.cursor.pos += 1;
            let name = self.cursor.token()?;
            self.cursor.cfws()?;
            if self.cursor.bytes.get(self.cursor.pos) != Some(&b'=') {
                return Err(self.cursor.error());
            }
            self.cursor.pos += 1;
            let mut value = self.cursor.value()?;
            value.extended = name.contains(&b'*');
            Ok(Some(Parameter { name, value }))
        })();
        match result {
            Ok(Some(p)) => Some(Ok(p)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
impl core::iter::FusedIterator for Parameters<'_> {}
