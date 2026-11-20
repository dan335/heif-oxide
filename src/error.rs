//! Error type for HEIF decoding.

/// Errors produced while decoding a HEIF/HEIC file.
///
/// Malformed or unsupported input always surfaces as an `Err` — the decoder
/// never panics on untrusted bytes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HeifError {
    /// Reading the file from disk failed.
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),

    /// The file is not an ISOBMFF/HEIF file at all, or its `ftyp` box lists
    /// no brand we recognize. The string is the major brand found (or a
    /// description of what was wrong).
    #[error("not a HEIF/HEIC file: {0}")]
    NotHeif(String),

    /// A box or payload ended before its declared size — the file is
    /// truncated or its box sizes are corrupt. The string names the
    /// structure being read when the data ran out.
    #[error("truncated or corrupt file while reading {0}")]
    Truncated(&'static str),

    /// A structurally required box is missing (e.g. `meta`, `pitm`, `iloc`).
    #[error("required box missing: {0}")]
    MissingBox(&'static str),

    /// The primary image item uses a codec other than HEVC (e.g. `av01`
    /// AVIF or `jpeg`). The string is the item type fourcc.
    #[error("unsupported codec: {0} (only HEVC/`hvc1` items are supported)")]
    UnsupportedCodec(String),

    /// The file parses but uses a HEIF feature this crate does not
    /// implement (e.g. external data references, protected items).
    #[error("unsupported HEIF feature: {0}")]
    Unsupported(String),

    /// A box's contents are self-inconsistent (bad counts, out-of-range
    /// indices, mismatched grid tiles, ...).
    #[error("invalid HEIF structure: {0}")]
    Invalid(String),

    /// The declared image dimensions exceed the configured pixel limit.
    #[error("image dimensions {width}x{height} exceed maximum {max_pixels} pixels")]
    ImageTooLarge {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
        /// The `Limits::max_pixels` in force.
        max_pixels: usize,
    },

    /// The HEVC codestream failed to decode, or decoded to something other
    /// than the size the container promised. A coded picture that exceeds
    /// the decode limits also ends up here.
    #[error("HEVC decode failed: {0}")]
    Codec(String),
}
