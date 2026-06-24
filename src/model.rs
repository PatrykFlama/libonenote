use crate::{Error, LoadOptions, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// A loaded OneNote document and its editable, owned notebook model.
#[derive(Debug)]
pub struct Document {
    source: Source,
    notebook: Notebook,
    original_notebook: Option<Notebook>,
    diagnostics: Vec<Diagnostic>,
    original_bytes: Option<Vec<u8>>,
    load_options: LoadOptions,
    dirty: bool,
}

impl Document {
    pub(crate) fn loaded(
        source: Source,
        notebook: Notebook,
        diagnostics: Vec<Diagnostic>,
        original_bytes: Option<Vec<u8>>,
        load_options: LoadOptions,
    ) -> Self {
        Self {
            source,
            original_notebook: original_bytes.as_ref().map(|_| notebook.clone()),
            notebook,
            diagnostics,
            original_bytes,
            load_options,
            dirty: false,
        }
    }

    /// Create a new in-memory notebook.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            source: Source {
                path: None,
                format: SourceFormat::InMemory,
            },
            notebook: Notebook {
                name: name.into(),
                color: None,
                entries: Vec::new(),
            },
            original_notebook: None,
            diagnostics: Vec::new(),
            original_bytes: None,
            load_options: LoadOptions::default(),
            dirty: true,
        }
    }

    /// Information about the document's source.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The parsed notebook.
    pub fn notebook(&self) -> &Notebook {
        &self.notebook
    }

    /// Edit the owned notebook model.
    ///
    /// Calling this method marks the document as modified, even if the closure
    /// ultimately makes no changes.
    pub fn edit<T>(&mut self, edit: impl FnOnce(&mut Notebook) -> T) -> T {
        self.dirty = true;
        edit(&mut self.notebook)
    }

    /// Non-fatal issues reported while loading.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether the owned model has been edited since loading.
    pub fn is_modified(&self) -> bool {
        self.dirty
    }

    /// Capabilities currently available for this document.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            edit_model: true,
            save_native: !self.dirty && self.original_bytes.is_some(),
            save_modified_native: self.original_bytes.is_some()
                && crate::native::supports_modified_native(self.source.format),
        }
    }

    /// Serialize the high-level notebook model as JSON.
    ///
    /// Binary payloads are represented by metadata and are not embedded in the
    /// JSON output.
    pub fn to_json(&self, pretty: bool) -> Result<String> {
        if pretty {
            Ok(serde_json::to_string_pretty(&self.notebook)?)
        } else {
            Ok(serde_json::to_string(&self.notebook)?)
        }
    }

    /// Save an exact copy of the original `.one` or `.onepkg` bytes.
    ///
    /// Modified `.one` sections and `.onepkg` packages currently support
    /// verified page-title and paragraph changes that fit existing property
    /// allocations. Every other edit is rejected.
    pub fn save_native(&self, path: impl AsRef<Path>) -> Result<()> {
        if self.source.format == SourceFormat::NotebookIndex {
            return Err(Error::MultiFileNotebookSaveUnsupported);
        }
        let original = self
            .original_bytes
            .as_deref()
            .ok_or(Error::NoOriginalNativeData)?;
        if !self.dirty {
            fs::write(path, original)?;
            return Ok(());
        }
        let baseline = self
            .original_notebook
            .as_ref()
            .ok_or(Error::NoOriginalNativeData)?;
        let bytes = match self.source.format {
            SourceFormat::Section => {
                crate::native::write_modified_section(original, baseline, &self.notebook)?
            }
            SourceFormat::Package => crate::native::write_modified_package(
                original,
                baseline,
                &self.notebook,
                self.load_options,
            )?,
            SourceFormat::NotebookIndex => return Err(Error::MultiFileNotebookSaveUnsupported),
            SourceFormat::InMemory => return Err(Error::NoOriginalNativeData),
        };
        self.write_and_verify(path.as_ref(), &bytes)
    }

    fn write_and_verify(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let temporary = temporary_native_path(path, self.source.format);
        fs::write(&temporary, bytes)?;
        let mut verification_options = self.load_options;
        verification_options.preserve_original = false;
        let verification = crate::Loader::new()
            .options(verification_options)
            .open(&temporary)
            .and_then(|document| match self.source.format {
                SourceFormat::Section => {
                    crate::native::verify_modified_section(&self.notebook, document.notebook())
                }
                SourceFormat::Package => {
                    crate::native::verify_modified_package(&self.notebook, document.notebook())
                }
                SourceFormat::NotebookIndex | SourceFormat::InMemory => {
                    Err(Error::NativeWriteUnsupported)
                }
            });
        if let Err(error) = verification {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        fs::rename(temporary, path)?;
        Ok(())
    }
}

fn temporary_native_path(path: &Path, format: SourceFormat) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("section");
    let extension = match format {
        SourceFormat::Package => "onepkg",
        SourceFormat::Section | SourceFormat::NotebookIndex | SourceFormat::InMemory => "one",
    };
    parent.join(format!(
        ".{stem}.native-write-{}.{extension}",
        std::process::id()
    ))
}

/// Features available on a loaded document.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Capabilities {
    /// The owned in-memory model can be changed.
    pub edit_model: bool,
    /// The current document can be saved to a native file.
    pub save_native: bool,
    /// Modified documents can be encoded as native OneNote files.
    pub save_modified_native: bool,
}

/// Description of the input used to load a document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Source {
    /// Original path, if the document was loaded from disk.
    pub path: Option<PathBuf>,
    /// OneNote container type.
    pub format: SourceFormat,
}

/// Supported source container types.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    /// A `.one` section file.
    Section,
    /// A `.onepkg` notebook package.
    Package,
    /// A `.onetoc2` index plus adjacent section files.
    NotebookIndex,
    /// A document created directly through the API.
    InMemory,
}

/// A complete notebook.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Notebook {
    /// Notebook display name.
    pub name: String,
    /// Notebook color, when available.
    pub color: Option<Color>,
    /// Sections and section groups in notebook order.
    pub entries: Vec<NotebookEntry>,
}

impl Notebook {
    /// Iterate over all sections recursively.
    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.entries.iter().flat_map(NotebookEntry::sections)
    }

    /// Iterate over all pages recursively.
    pub fn pages(&self) -> impl Iterator<Item = &Page> {
        self.sections().flat_map(|section| section.pages.iter())
    }
}

/// A section or a nested group of sections.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotebookEntry {
    /// A section containing pages.
    Section(Section),
    /// A named group containing sections or more groups.
    SectionGroup(SectionGroup),
}

impl NotebookEntry {
    fn sections(&self) -> Box<dyn Iterator<Item = &Section> + '_> {
        match self {
            Self::Section(section) => Box::new(std::iter::once(section)),
            Self::SectionGroup(group) => {
                Box::new(group.entries.iter().flat_map(NotebookEntry::sections))
            }
        }
    }
}

/// A notebook section.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Section {
    /// Section display name.
    pub name: String,
    /// Section color, when available.
    pub color: Option<Color>,
    /// Pages in display order.
    pub pages: Vec<Page>,
}

/// A nested section group.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SectionGroup {
    /// Group display name.
    pub name: String,
    /// Child sections and groups.
    pub entries: Vec<NotebookEntry>,
}

/// A OneNote page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Page {
    /// Stable OneNote page identifier.
    pub id: String,
    /// Page title.
    pub title: String,
    /// Nesting level used for subpages.
    pub level: i32,
    /// Page author.
    pub author: Option<String>,
    /// Creation timestamp in RFC 3339 form when representable.
    pub created_at: String,
    /// Last modification timestamp in RFC 3339 form when representable.
    pub updated_at: String,
    /// Page height in OneNote half-inch units.
    pub height: Option<f32>,
    /// Positioned page blocks.
    pub blocks: Vec<PageBlock>,
}

/// Top-level content positioned on a page.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageBlock {
    /// A positioned outline.
    Outline(Outline),
    /// A positioned image.
    Image(Image),
    /// A positioned file attachment.
    Attachment(Attachment),
    /// Pen or handwriting strokes.
    Ink(Ink),
    /// Content not currently understood by the API.
    Unknown,
}

/// A positioned outline containing hierarchical elements.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Outline {
    /// OneNote child level.
    pub level: u8,
    /// Position and size metadata.
    pub layout: Layout,
    /// Outline elements and groups.
    pub items: Vec<OutlineItem>,
}

/// An outline item.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutlineItem {
    /// A content element.
    Element(OutlineElement),
    /// A nested group with an explicit level.
    Group(OutlineGroup),
}

/// A nested outline group.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutlineGroup {
    /// Nesting level.
    pub level: u8,
    /// Child items.
    pub items: Vec<OutlineItem>,
}

/// A content-bearing outline element.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutlineElement {
    /// Nesting level.
    pub level: u8,
    /// List definitions associated with the element.
    pub lists: Vec<ListDefinition>,
    /// Rich content in this element.
    pub content: Vec<Content>,
    /// Nested outline items.
    pub children: Vec<OutlineItem>,
}

/// Content contained by an outline or table cell.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// A formatted paragraph.
    Paragraph(Paragraph),
    /// A table.
    Table(Table),
    /// An image.
    Image(Image),
    /// A file attachment.
    Attachment(Attachment),
    /// Pen or handwriting strokes.
    Ink(Ink),
    /// Content not currently understood by the API.
    Unknown,
}

/// A formatted paragraph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Paragraph {
    /// Original paragraph text, including hidden OneNote marker runs.
    pub text: String,
    /// Base paragraph style.
    pub style: TextStyle,
    /// Styled text runs.
    pub runs: Vec<TextRun>,
    /// Paragraph alignment.
    pub alignment: TextAlignment,
    /// Space before the paragraph.
    pub space_before: f32,
    /// Space after the paragraph.
    pub space_after: f32,
}

/// A formatted text run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextRun {
    /// Text covered by this run.
    pub text: String,
    /// Run formatting.
    pub style: TextStyle,
}

/// Text formatting shared by paragraph and run styles.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TextStyle {
    /// Bold formatting.
    pub bold: bool,
    /// Italic formatting.
    pub italic: bool,
    /// Underline formatting.
    pub underline: bool,
    /// Strike-through formatting.
    pub strikethrough: bool,
    /// Superscript formatting.
    pub superscript: bool,
    /// Subscript formatting.
    pub subscript: bool,
    /// Hidden OneNote marker text.
    pub hidden: bool,
    /// Math-expression formatting.
    pub math: bool,
    /// Hyperlink formatting.
    pub hyperlink: bool,
    /// Visible protected hyperlink run.
    pub hyperlink_protected: bool,
    /// Font family.
    pub font: Option<String>,
    /// Font size in half-point units, matching the native representation.
    pub font_size_half_points: Option<u16>,
    /// Foreground color.
    pub color: Option<ColorReference>,
    /// Highlight color.
    pub highlight: Option<ColorReference>,
    /// OneNote style identifier.
    pub style_id: Option<String>,
    /// LCID language code.
    pub language_code: Option<u32>,
}

/// Paragraph text alignment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlignment {
    /// Left aligned.
    #[default]
    Left,
    /// Centered.
    Center,
    /// Right aligned.
    Right,
    /// Unrecognized native value.
    Unknown,
}

/// A list definition.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListDefinition {
    /// Native format pattern.
    pub format: String,
    /// Optional restart index.
    pub restart: Option<i32>,
    /// Symbol font.
    pub list_font: Option<String>,
    /// Whether the index is bold.
    pub bold: bool,
    /// Whether the index is italic.
    pub italic: bool,
}

/// A table.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Table {
    /// Declared row count.
    pub rows: u32,
    /// Declared column count.
    pub columns: u32,
    /// Column widths in OneNote half-inch units.
    pub column_widths: Vec<f32>,
    /// Whether borders are visible.
    pub borders_visible: bool,
    /// Table rows.
    pub content: Vec<TableRow>,
}

/// A table row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableRow {
    /// Cells in this row.
    pub cells: Vec<TableCell>,
}

/// A table cell.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableCell {
    /// Cell background color.
    pub background: Option<Color>,
    /// Cell outline elements.
    pub content: Vec<OutlineElement>,
}

/// An embedded image.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Image {
    /// Original file name when present.
    pub filename: Option<String>,
    /// File extension reported by OneNote.
    pub extension: Option<String>,
    /// Alternative or OCR text.
    pub alt_text: Option<String>,
    /// Associated hyperlink.
    pub hyperlink: Option<String>,
    /// Whether the image is a page background.
    pub background: bool,
    /// Position and size metadata.
    pub layout: Layout,
    /// Binary payload metadata and optionally loaded bytes.
    pub blob: Blob,
}

/// An embedded file attachment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attachment {
    /// Untrusted original filename.
    pub filename: String,
    /// Position and size metadata.
    pub layout: Layout,
    /// Binary payload metadata and optionally loaded bytes.
    pub blob: Blob,
}

/// Metadata and optionally loaded bytes for a binary object.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Blob {
    /// Declared byte length.
    pub size: u64,
    /// Whether bytes were loaded according to [`crate::BinaryDataPolicy`].
    pub loaded: bool,
    #[serde(skip)]
    pub(crate) bytes: Option<Vec<u8>>,
}

impl Blob {
    pub(crate) fn new(size: u64, bytes: Option<Vec<u8>>) -> Self {
        Self {
            size,
            loaded: bytes.is_some(),
            bytes,
        }
    }

    /// Loaded bytes, or `None` when loading was disabled or size-limited.
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }
}

/// Pen and handwriting data.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ink {
    /// Position metadata.
    pub layout: Layout,
    /// Whether complete point paths were loaded.
    pub loaded: bool,
    /// Total direct and nested stroke count.
    pub stroke_count: usize,
    /// Direct strokes in this object.
    pub strokes: Vec<InkStroke>,
    /// Nested ink groups.
    pub groups: Vec<Ink>,
}

/// One pen stroke.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InkStroke {
    /// Stroke points.
    pub points: Vec<Point>,
    /// Native pen-tip value.
    pub pen_tip: Option<u8>,
    /// Native transparency value.
    pub transparency: Option<u8>,
    /// Stroke width.
    pub width: f32,
    /// Stroke height.
    pub height: f32,
    /// Native packed color value.
    pub color: Option<u32>,
}

/// Position and optional dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Layout {
    /// Horizontal offset.
    pub x: Option<f32>,
    /// Vertical offset.
    pub y: Option<f32>,
    /// Width or maximum width.
    pub width: Option<f32>,
    /// Height or maximum height.
    pub height: Option<f32>,
}

/// A two-dimensional point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Point {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
}

/// RGBA color.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Color {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
    /// Alpha channel.
    pub alpha: u8,
}

/// A native color reference which can defer to application defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ColorReference {
    /// Application-defined automatic color.
    Automatic,
    /// Explicit RGB color.
    Rgb {
        /// Red channel.
        red: u8,
        /// Green channel.
        green: u8,
        /// Blue channel.
        blue: u8,
    },
}

/// A non-fatal issue encountered while parsing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Optional page identifier.
    pub page_id: Option<String>,
    /// Optional page title.
    pub page_title: Option<String>,
    /// Human-readable warning.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notebook() -> Notebook {
        Notebook {
            name: "Test".to_owned(),
            color: None,
            entries: Vec::new(),
        }
    }

    #[test]
    fn unchanged_native_data_is_copied_exactly() {
        let document = Document::loaded(
            Source {
                path: Some(PathBuf::from("test.one")),
                format: SourceFormat::Section,
            },
            notebook(),
            Vec::new(),
            Some(vec![1, 2, 3, 4]),
            LoadOptions::default(),
        );
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("copy.one");

        document.save_native(&output).unwrap();

        assert_eq!(fs::read(output).unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn edited_document_refuses_native_save() {
        let mut document = Document::loaded(
            Source {
                path: Some(PathBuf::from("test.one")),
                format: SourceFormat::Section,
            },
            notebook(),
            Vec::new(),
            Some(vec![1, 2, 3, 4]),
            LoadOptions::default(),
        );
        document.edit(|notebook| notebook.name = "Changed".to_owned());
        let directory = tempfile::tempdir().unwrap();

        assert!(matches!(
            document.save_native(directory.path().join("copy.one")),
            Err(Error::NativeWriteUnsupported)
        ));
    }
}
