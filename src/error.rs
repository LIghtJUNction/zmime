//! Explicit parsing failures and resource budgets.
use core::fmt;

/// Resource whose configured budget was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    /// Total input size.
    MessageBytes,
    /// Bytes of one part's header block.
    HeaderBytes,
    /// Fields in one part's header block.
    Headers,
    /// Bytes on one physical header line.
    LineBytes,
    /// Number of tree nodes, including containers and the root.
    Parts,
    /// Tree depth; the root has depth zero.
    Depth,
    /// Nested comments in a structured field.
    CommentDepth,
}

/// Features whose raw bytes remain accessible but cannot be interpreted here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsupported {
    /// Transfer-encoded message/rfc822 needs a separate decoded input buffer.
    EncodedMessage,
    /// Multipart bodies must use an identity transfer encoding.
    EncodedMultipart,
    /// RFC 2231 extended or continued parameter decoding.
    ExtendedParameter,
}

/// Parsing error. Offsets are bytes relative to the API's input slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A resource budget was exceeded.
    LimitExceeded {
        /// Budget that was exceeded.
        limit: Limit,
        /// First offending byte or position.
        offset: usize,
    },
    /// Invalid field name, missing colon, or orphan continuation line.
    InvalidHeader {
        /// Offending byte position.
        offset: usize,
    },
    /// A forbidden control byte or bare CR occurred in a header.
    InvalidHeaderByte {
        /// Offending byte position.
        offset: usize,
    },
    /// Nonempty headers reached EOF before a blank separator line.
    MissingHeaderSeparator,
    /// Malformed media type, disposition, or parameter syntax.
    InvalidMime {
        /// Offending byte position relative to the public API's input slice.
        offset: usize,
    },
    /// Boundary is absent, empty, longer than 70 bytes, or has invalid bytes.
    InvalidBoundary,
    /// A multipart was never closed, including premature outer delimiters.
    UnclosedMultipart {
        /// Position where the missing close was detected.
        offset: usize,
    },
    /// Multiple Content-Type, Content-Disposition, or transfer encoding fields.
    DuplicateMimeHeader,
    /// Caller-provided part storage cannot hold the complete tree.
    WorkspaceTooSmall {
        /// Minimum capacity needed at this point.
        needed: usize,
    },
    /// Caller-provided byte output cannot hold the complete result.
    OutputTooSmall {
        /// Exact required byte length.
        needed: usize,
    },
    /// The requested interpretation is outside the documented subset.
    Unsupported(Unsupported),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MIME parse error: {self:?}")
    }
}

/// Budgets checked before consuming storage or descending the MIME tree.
///
/// Line limits are deliberately more permissive than RFC 5322's 998 octets;
/// applications may set `max_line_len` to 998 for that restriction. A zero
/// depth limit permits only a single root part. Output decoding is bounded by
/// the caller's output slice independently of these parsing limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum complete input length (default 64 MiB).
    pub max_message_len: usize,
    /// Maximum header bytes per part (default 256 KiB).
    pub max_header_len: usize,
    /// Maximum fields per part (default 256).
    pub max_headers: usize,
    /// Maximum bytes per physical header line, excluding CRLF (default 64 KiB).
    pub max_line_len: usize,
    /// Maximum total parts (default 1024).
    pub max_parts: usize,
    /// Maximum tree depth (default 32).
    pub max_depth: usize,
}

impl Limits {
    /// Conservative bounded defaults for untrusted mail.
    pub const DEFAULT: Self = Self {
        max_message_len: 64 * 1024 * 1024,
        max_header_len: 256 * 1024,
        max_headers: 256,
        max_line_len: 64 * 1024,
        max_parts: 1024,
        max_depth: 32,
    };
}
impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
