//! Borrowed MIME message trees, bounded caller-owned workspace, and lazy decoding.
//!
//! The core uses no allocator, runtime, or third-party crates. See [`Message`],
//! [`HeaderBlock`], [`Limits`], and [`decode_base64`].
#![no_std]
#![cfg_attr(not(feature = "simd"), forbid(unsafe_code))]
#![doc = include_str!("../README.md")]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod cpu;
pub mod decode;
mod error;
mod headers;
mod message;
mod mime;
mod scan;

pub use decode::{
    DecodeError, TransferEncoding, decode_base64, decode_encoded_words, decode_quoted_printable,
    decode_text, decode_transfer,
};
pub use error::{Error, Limit, Limits, Unsupported};
pub use headers::{Header, HeaderBlock, Headers};
pub use message::{Message, Part};
pub use mime::{ContentDisposition, ContentType, Parameter, ParameterValue, Parameters};
