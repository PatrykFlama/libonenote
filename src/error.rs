use std::path::PathBuf;

/// Result type returned by `libonenote`.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while loading or saving OneNote data.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input has no supported OneNote extension.
    #[error("unsupported OneNote input: {0}")]
    UnsupportedInput(PathBuf),

    /// The underlying OneNote parser rejected the file.
    #[error("failed to parse OneNote data: {0}")]
    Parse(#[from] onenote_parser::errors::Error),

    /// A filesystem operation failed.
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),

    /// A binary object could not be read from the parsed document.
    #[error("failed to read embedded binary data: {0}")]
    BinaryData(String),

    /// The requested modified-native operation is not implemented.
    #[error("this modified native OneNote operation is not supported")]
    NativeWriteUnsupported,

    /// A native edit needs a larger property or would resize a single-byte
    /// property, which requires allocating a new revision-store object.
    #[error(
        "this native edit needs a larger or resized single-byte property; revision allocation is \
         not implemented"
    )]
    NativeWriteSizeChangeUnsupported,

    /// The edit changes between a legacy single-byte string and UTF-16.
    #[error("this native edit changes the native text encoding")]
    NativeWriteEncodingChangeUnsupported,

    /// The original native property bytes could not be located.
    #[error("native text property not found for {0:?}")]
    NativeTextNotFound(String),

    /// The generated section parsed successfully but did not reproduce the
    /// requested high-level page model.
    #[error("native write verification failed")]
    NativeWriteVerificationFailed,

    /// A section in a package could not be mapped uniquely to the parsed
    /// notebook model.
    #[error("cannot map native package section: {0}")]
    NativePackageSectionMapping(String),

    /// A `.onetoc2` notebook consists of several files and cannot be copied as
    /// one output file.
    #[error("a .onetoc2 notebook cannot be saved as one native file")]
    MultiFileNotebookSaveUnsupported,

    /// The document was created in memory and has no original native bytes.
    #[error("the document has no original native representation")]
    NoOriginalNativeData,

    /// A Graph export would lose content under the selected write policy.
    #[error("cannot write page {page:?} to Microsoft Graph without losing {content}")]
    GraphWriteUnsupported {
        /// Page title containing the unsupported content.
        page: String,
        /// Human-readable content kind.
        content: &'static str,
    },

    /// A Graph export needs binary data that was not loaded.
    #[error("binary data for {0:?} was not loaded; reopen with BinaryDataPolicy::All")]
    MissingBinaryData(String),

    /// JSON serialization failed.
    #[error("failed to serialize document: {0}")]
    Json(#[from] serde_json::Error),
}
