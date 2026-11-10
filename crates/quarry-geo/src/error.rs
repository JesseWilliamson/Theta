//! The single error type shared by every decoder in this crate.

use std::fmt;

/// Why a geometry could not be decoded.
///
/// Offsets are byte offsets into the input (for hex input, into the hex text).
/// Decoders return these instead of panicking, whatever the input looks like.
#[derive(Debug, Clone, PartialEq)]
pub enum GeoError {
    /// The binary input ended before a complete value could be read.
    Truncated {
        /// Byte offset at which more data was expected.
        offset: usize,
    },
    /// The byte-order marker was neither 0 (big endian) nor 1 (little endian).
    BadByteOrder(u8),
    /// The geometry type code is not one of the supported 1..=7 (+ dimension offsets).
    UnknownType(u32),
    /// A declared element count cannot possibly fit in the remaining input.
    /// Checked *before* allocating so hostile input cannot force huge allocations.
    BadCount {
        /// Byte offset of the count field.
        offset: usize,
        /// The declared count.
        count: u64,
    },
    /// Geometry collections are nested deeper than the decoder allows.
    TooDeep,
    /// Hex text contained a non-hex character or had an odd length.
    InvalidHex {
        /// Offset of the offending character (or the end for odd length).
        offset: usize,
    },
    /// Structurally invalid WKB that is not covered by a more specific variant.
    InvalidWkb(String),
    /// Syntax error in WKT/EWKT.
    InvalidWkt {
        /// Byte offset of the error in the text.
        offset: usize,
        /// What was wrong there.
        message: String,
    },
    /// Malformed or unsupported GeoJSON.
    InvalidGeoJson(String),
}

impl fmt::Display for GeoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeoError::Truncated { offset } => write!(f, "truncated input at byte {offset}"),
            GeoError::BadByteOrder(b) => write!(f, "bad byte order marker {b:#04x} (expected 0 or 1)"),
            GeoError::UnknownType(c) => write!(f, "unknown geometry type code {c}"),
            GeoError::BadCount { offset, count } => {
                write!(f, "declared element count {count} at byte {offset} exceeds the remaining input")
            }
            GeoError::TooDeep => write!(f, "geometry nesting is too deep"),
            GeoError::InvalidHex { offset } => write!(f, "invalid hex at offset {offset}"),
            GeoError::InvalidWkb(m) => write!(f, "invalid WKB: {m}"),
            GeoError::InvalidWkt { offset, message } => write!(f, "invalid WKT at offset {offset}: {message}"),
            GeoError::InvalidGeoJson(m) => write!(f, "invalid GeoJSON: {m}"),
        }
    }
}

impl std::error::Error for GeoError {}
