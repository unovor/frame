//! A small [CBOR] codec suitable for `no_std` environments.
//!
//! The crate is organised around the following entities:
//!
//! - [`Encoder`] and [`Decoder`] for type-directed encoding and decoding
//! of values.
//!
//! - [`Encode`] and [`Decode`] traits which can be implemented for any
//! type that should be encoded to or decoded from CBOR. They are similar
//! to [serde]'s `Serialize` and `Deserialize` traits but do not abstract
//! over the encoder/decoder.
//!
//! As mentioned, encoding and decoding proceeds in a type-directed way, i.e.
//! by calling methods for expected data item types, e.g. [`Decoder::u32`]
//! or [`Encoder::str`]. In addition there is support for data type
//! inspection. The `Decoder` can be queried for the current data type
//! which returns a [`data::Type`] that can represent every possible CBOR type
//! and decoding can thus proceed based on this information.
//!
//! Optionally, `Encode` and `Decode` can be derived for structs and enums
//! using the respective derive macros. See [`minicbor_derive`] for details.
//!
//! # Example: generic encoding and decoding
//!
//! ```
//! use minicbor::{Encode, Decode};
//!
//! let input = ["hello", "world"];
//! let mut buffer = [0u8; 128];
//!
//! minicbor::encode(&input, buffer.as_mut())?;
//! let output: [&str; 2] = minicbor::decode(buffer.as_ref())?;
//! assert_eq!(input, output);
//!
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! # Example: ad-hoc encoding
//!
//! ```
//! use minicbor::Encoder;
//!
//! let mut buffer = [0u8; 128];
//! let mut encoder = Encoder::new(&mut buffer[..]);
//!
//! encoder.begin_map()? // using an indefinite map here
//!     .str("hello")?.str("world")?
//!     .str("submap")?.map(2)?
//!         .u8(1)?.bool(true)?
//!         .u8(2)?.bool(false)?
//!     .u16(34234)?.array(3)?.u8(1)?.u8(2)?.u8(3)?
//!     .bool(true)?.null()?
//! .end()?;
//!
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! # Example: ad-hoc decoding
//!
//! ```
//! use minicbor::{data, Decoder};
//!
//! let input = [
//!     0xc0, 0x74, 0x32, 0x30, 0x31, 0x33, 0x2d, 0x30,
//!     0x33, 0x2d, 0x32, 0x31, 0x54, 0x32, 0x30, 0x3a,
//!     0x30, 0x34, 0x3a, 0x30, 0x30, 0x5a
//! ];
//! let mut decoder = Decoder::new(&input[..]);
//! assert_eq!(data::Tag::DateTime, decoder.tag()?);
//! assert_eq!("2013-03-21T20:04:00Z", decoder.str()?);
//!
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! [CBOR]: https://tools.ietf.org/html/rfc7049
//! [serde]: https://serde.rs

#![forbid(unsafe_code, unused_imports, unused_variables)]
#![cfg_attr(not(feature = "std"), no_std)]

pub mod data;
pub mod decode;
pub mod encode;

const UNSIGNED: u8 = 0x00;
const SIGNED: u8   = 0x20;
const BYTES: u8    = 0x40;
const TEXT: u8     = 0x60;
const ARRAY: u8    = 0x80;
const MAP: u8      = 0xa0;
const TAGGED: u8   = 0xc0;
const SIMPLE: u8   = 0xe0;
const BREAK: u8    = 0xff;

pub use decode::{Decode, Decoder};
pub use encode::{Encode, Encoder};

#[cfg(feature = "derive")]
pub use minicbor_derive::*;

/// Decode a type implementing [`Decode`] from the given byte slice.
pub fn decode<'b, T>(b: &'b [u8]) -> Result<T, decode::Error>
where
    T: Decode<'b>
{
    Decoder::new(b).decode()
}

/// Encode a type implementing [`Encode`] to the given [`encode::Write`] impl.
pub fn encode<T, W>(x: T, w: W) -> Result<(), encode::Error<W::Error>>
where
    T: Encode,
    W: encode::Write
{
    Encoder::new(w).encode(x)?.ok()
}

/// Encode a type implementing [`Encode`] and return the encoded byte vector.
///
/// Only available with feature `std`.
#[cfg(feature = "std")]
pub fn to_vec<T>(x: T) -> Result<Vec<u8>, encode::Error<std::io::Error>>
where
    T: Encode
{
    let mut e = Encoder::new(Vec::new());
    x.encode(&mut e)?;
    Ok(e.into_inner())
}

