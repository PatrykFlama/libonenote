use crate::model::{
    Attachment, Blob, Color, ColorReference, Content, Diagnostic, Document, Image, Ink, InkStroke,
    Layout, ListDefinition, Notebook, NotebookEntry, Outline, OutlineElement, OutlineGroup,
    OutlineItem, Page, PageBlock, Paragraph, Point, Section, SectionGroup, Source, SourceFormat,
    Table, TableCell, TableRow, TextAlignment, TextRun, TextStyle,
};
use crate::{Error, Result};
use onenote_parser::Parser;
use onenote_parser::contents::{
    Content as NativeContent, EmbeddedFile, Image as NativeImage, Ink as NativeInk,
    Outline as NativeOutline, OutlineElement as NativeOutlineElement,
    OutlineItem as NativeOutlineItem, ParagraphStyling, RichText, Table as NativeTable,
};
use onenote_parser::page::{Page as NativePage, PageContent};
use onenote_parser::property::common::{Color as NativeColor, ColorRef};
use onenote_parser::property::rich_text::ParagraphAlignment;
use onenote_parser::section::{
    Section as NativeSection, SectionEntry as NativeSectionEntry,
    SectionGroup as NativeSectionGroup,
};
use onenote_parser::warn::Report;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use typed_path::NativePath;

/// Policy controlling whether image and attachment bytes are materialized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BinaryDataPolicy {
    /// Keep only names and byte lengths.
    #[default]
    MetadataOnly,
    /// Load binary objects no larger than the given number of bytes.
    UpTo(u64),
    /// Load every binary object into memory.
    All,
}

impl BinaryDataPolicy {
    fn should_load(self, size: u64) -> bool {
        match self {
            Self::MetadataOnly => false,
            Self::UpTo(limit) => size <= limit,
            Self::All => true,
        }
    }
}

/// Policy controlling whether complete ink stroke paths are materialized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InkDataPolicy {
    /// Keep ink layout and counts without copying every point.
    #[default]
    MetadataOnly,
    /// Load complete stroke paths.
    All,
}

/// Options used when loading a document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoadOptions {
    /// Binary payload loading policy.
    pub binary_data: BinaryDataPolicy,
    /// Ink stroke loading policy.
    pub ink_data: InkDataPolicy,
    /// Keep original `.one` or `.onepkg` bytes for exact unchanged saving.
    pub preserve_original: bool,
}

/// Configurable OneNote document loader.
#[derive(Clone, Copy, Debug, Default)]
pub struct Loader {
    options: LoadOptions,
}

impl Loader {
    /// Create a loader with metadata-only binary handling.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set all loading options.
    pub fn options(mut self, options: LoadOptions) -> Self {
        self.options = options;
        self
    }

    /// Set the binary payload policy.
    pub fn binary_data(mut self, policy: BinaryDataPolicy) -> Self {
        self.options.binary_data = policy;
        self
    }

    /// Set the ink stroke loading policy.
    pub fn ink_data(mut self, policy: InkDataPolicy) -> Self {
        self.options.ink_data = policy;
        self
    }

    /// Preserve original package bytes for exact unchanged saving.
    pub fn preserve_original(mut self, preserve: bool) -> Self {
        self.options.preserve_original = preserve;
        self
    }

    /// Load a `.one`, `.onepkg`, or `.onetoc2` file.
    pub fn open(self, path: impl AsRef<Path>) -> Result<Document> {
        let path = path.as_ref();
        let format = source_format(path)?;
        let typed = NativePath::new(path.as_os_str().as_encoded_bytes()).to_typed_path();
        let parser = Parser::new();
        let name = file_stem(path);
        let mut diagnostics = Vec::new();

        let notebook = match format {
            SourceFormat::Section => {
                let section = parser.parse_section(typed)?;
                Notebook {
                    name: name.clone(),
                    color: None,
                    entries: vec![NotebookEntry::Section(map_section(
                        &section,
                        self.options.binary_data,
                        self.options.ink_data,
                        &mut diagnostics,
                    )?)],
                }
            }
            SourceFormat::Package => {
                let notebook = parser.parse_package(typed)?;
                collect_report(notebook.report(), &mut diagnostics);
                map_notebook(
                    &name,
                    notebook.entries(),
                    notebook.color(),
                    self.options.binary_data,
                    self.options.ink_data,
                    &mut diagnostics,
                )?
            }
            SourceFormat::NotebookIndex => {
                let notebook = parser.parse_notebook(typed)?;
                collect_report(notebook.report(), &mut diagnostics);
                map_notebook(
                    &name,
                    notebook.entries(),
                    notebook.color(),
                    self.options.binary_data,
                    self.options.ink_data,
                    &mut diagnostics,
                )?
            }
            SourceFormat::InMemory => unreachable!("in-memory sources are not opened from disk"),
        };

        let original_bytes = if self.options.preserve_original
            && matches!(format, SourceFormat::Section | SourceFormat::Package)
        {
            Some(std::fs::read(path)?)
        } else {
            None
        };

        Ok(Document::loaded(
            Source {
                path: Some(path.to_path_buf()),
                format,
            },
            notebook,
            diagnostics,
            original_bytes,
            self.options,
        ))
    }
}

/// Load a OneNote file with default metadata-only options.
pub fn open(path: impl AsRef<Path>) -> Result<Document> {
    Loader::new().open(path)
}

pub(crate) fn load_section_buffer(
    data: &[u8],
    file_name: &str,
    options: LoadOptions,
) -> Result<Section> {
    let parser = Parser::new();
    let section = parser.parse_section_buffer(
        data,
        typed_path::UnixPath::new(file_name.as_bytes()).to_typed_path(),
    )?;
    let mut diagnostics = Vec::new();
    map_section(
        &section,
        options.binary_data,
        options.ink_data,
        &mut diagnostics,
    )
}

fn source_format(path: &Path) -> Result<SourceFormat> {
    match path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("one") => Ok(SourceFormat::Section),
        Some("onepkg") => Ok(SourceFormat::Package),
        Some("onetoc2") => Ok(SourceFormat::NotebookIndex),
        _ => Err(Error::UnsupportedInput(path.to_path_buf())),
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("Notebook")
        .to_owned()
}

fn map_notebook(
    name: &str,
    entries: &[NativeSectionEntry],
    color: Option<NativeColor>,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Notebook> {
    Ok(Notebook {
        name: name.to_owned(),
        color: color.map(map_color),
        entries: entries
            .iter()
            .map(|entry| map_entry(entry, policy, ink_policy, diagnostics))
            .collect::<Result<_>>()?,
    })
}

fn map_entry(
    entry: &NativeSectionEntry,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<NotebookEntry> {
    match entry {
        NativeSectionEntry::Section(section) => Ok(NotebookEntry::Section(map_section(
            section,
            policy,
            ink_policy,
            diagnostics,
        )?)),
        NativeSectionEntry::SectionGroup(group) => Ok(NotebookEntry::SectionGroup(map_group(
            group,
            policy,
            ink_policy,
            diagnostics,
        )?)),
    }
}

fn map_group(
    group: &NativeSectionGroup,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<SectionGroup> {
    Ok(SectionGroup {
        name: group.display_name().to_owned(),
        entries: group
            .entries()
            .iter()
            .map(|entry| map_entry(entry, policy, ink_policy, diagnostics))
            .collect::<Result<_>>()?,
    })
}

fn map_section(
    section: &NativeSection,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Section> {
    collect_report(section.report(), diagnostics);
    Ok(Section {
        name: section.display_name().to_owned(),
        color: section.color().map(map_color),
        pages: section
            .page_series()
            .iter()
            .flat_map(|series| series.pages())
            .map(|page| map_page(page, policy, ink_policy))
            .collect::<Result<_>>()?,
    })
}

fn map_page(
    page: &NativePage,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<Page> {
    Ok(Page {
        id: page.link_target_id().to_owned(),
        title: page.title_text().unwrap_or("Untitled page").to_owned(),
        level: page.level(),
        author: page.author().map(str::to_owned),
        created_at: format_time(page.created_time()),
        updated_at: format_time(page.updated_time()),
        height: page.height(),
        blocks: page
            .contents()
            .iter()
            .map(|content| map_page_block(content, policy, ink_policy))
            .collect::<Result<_>>()?,
    })
}

fn format_time(value: time::UtcDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

fn map_page_block(
    content: &PageContent,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<PageBlock> {
    match content {
        PageContent::Outline(outline) => Ok(PageBlock::Outline(map_outline(
            outline, policy, ink_policy,
        )?)),
        PageContent::Image(image) => Ok(PageBlock::Image(map_image(image, policy)?)),
        PageContent::EmbeddedFile(file) => Ok(PageBlock::Attachment(map_attachment(file, policy)?)),
        PageContent::Ink(ink) => Ok(PageBlock::Ink(map_ink(ink, ink_policy))),
        PageContent::Unknown => Ok(PageBlock::Unknown),
    }
}

fn map_outline(
    outline: &NativeOutline,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<Outline> {
    Ok(Outline {
        level: outline.child_level(),
        layout: Layout {
            x: outline.offset_horizontal(),
            y: outline.offset_vertical(),
            width: outline.layout_max_width(),
            height: outline.layout_max_height(),
        },
        items: outline
            .items()
            .iter()
            .map(|item| map_outline_item(item, policy, ink_policy))
            .collect::<Result<_>>()?,
    })
}

fn map_outline_item(
    item: &NativeOutlineItem,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<OutlineItem> {
    match item {
        NativeOutlineItem::Element(element) => Ok(OutlineItem::Element(map_outline_element(
            element, policy, ink_policy,
        )?)),
        NativeOutlineItem::Group(group) => Ok(OutlineItem::Group(OutlineGroup {
            level: group.child_level(),
            items: group
                .outlines()
                .iter()
                .map(|item| map_outline_item(item, policy, ink_policy))
                .collect::<Result<_>>()?,
        })),
    }
}

fn map_outline_element(
    element: &NativeOutlineElement,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<OutlineElement> {
    Ok(OutlineElement {
        level: element.child_level(),
        lists: element
            .list_contents()
            .iter()
            .map(|list| ListDefinition {
                format: list.list_format().iter().collect(),
                restart: list.list_restart(),
                list_font: list.list_font().map(str::to_owned),
                bold: list.bold(),
                italic: list.italic(),
            })
            .collect(),
        content: element
            .contents()
            .iter()
            .map(|content| map_content(content, policy, ink_policy))
            .collect::<Result<_>>()?,
        children: element
            .children()
            .iter()
            .map(|child| map_outline_item(child, policy, ink_policy))
            .collect::<Result<_>>()?,
    })
}

fn map_content(
    content: &NativeContent,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<Content> {
    match content {
        NativeContent::RichText(text) => Ok(Content::Paragraph(map_paragraph(text))),
        NativeContent::Table(table) => Ok(Content::Table(map_table(table, policy, ink_policy)?)),
        NativeContent::Image(image) => Ok(Content::Image(map_image(image, policy)?)),
        NativeContent::EmbeddedFile(file) => Ok(Content::Attachment(map_attachment(file, policy)?)),
        NativeContent::Ink(ink) => Ok(Content::Ink(map_ink(ink, ink_policy))),
        NativeContent::Unknown => Ok(Content::Unknown),
    }
}

fn map_paragraph(text: &RichText) -> Paragraph {
    Paragraph {
        text: text.text().to_owned(),
        style: map_style(text.paragraph_style()),
        runs: split_runs(text),
        alignment: map_alignment(text.paragraph_alignment()),
        space_before: text.paragraph_space_before(),
        space_after: text.paragraph_space_after(),
    }
}

fn split_runs(text: &RichText) -> Vec<TextRun> {
    let styles = text.text_run_formatting();
    if styles.is_empty() {
        return vec![TextRun {
            text: text.text().to_owned(),
            style: map_style(text.paragraph_style()),
        }];
    }

    let utf16 = text.text().encode_utf16().collect::<Vec<_>>();
    let mut ends = text
        .text_run_indices()
        .iter()
        .map(|index| (*index as usize).min(utf16.len()))
        .collect::<Vec<_>>();
    if ends.last().copied() != Some(utf16.len()) {
        ends.push(utf16.len());
    }

    let mut start = 0;
    ends.into_iter()
        .enumerate()
        .map(|(index, end)| {
            let end = end.max(start);
            let style = styles
                .get(index)
                .or_else(|| styles.last())
                .unwrap_or(text.paragraph_style());
            let run = TextRun {
                text: String::from_utf16_lossy(&utf16[start..end]),
                style: map_style(style),
            };
            start = end;
            run
        })
        .collect()
}

fn map_style(style: &ParagraphStyling) -> TextStyle {
    TextStyle {
        bold: style.bold(),
        italic: style.italic(),
        underline: style.underline(),
        strikethrough: style.strikethrough(),
        superscript: style.superscript(),
        subscript: style.subscript(),
        hidden: style.hidden(),
        math: style.math_formatting(),
        hyperlink: style.hyperlink(),
        hyperlink_protected: style.hyperlink_protected(),
        font: style.font().map(str::to_owned),
        font_size_half_points: style.font_size(),
        color: style.font_color().map(map_color_ref),
        highlight: style.highlight().map(map_color_ref),
        style_id: style.style_id().map(str::to_owned),
        language_code: style.language_code(),
    }
}

fn map_alignment(alignment: ParagraphAlignment) -> TextAlignment {
    match alignment {
        ParagraphAlignment::Left => TextAlignment::Left,
        ParagraphAlignment::Center => TextAlignment::Center,
        ParagraphAlignment::Right => TextAlignment::Right,
        ParagraphAlignment::Unknown => TextAlignment::Unknown,
    }
}

fn map_table(
    table: &NativeTable,
    policy: BinaryDataPolicy,
    ink_policy: InkDataPolicy,
) -> Result<Table> {
    Ok(Table {
        rows: table.rows(),
        columns: table.cols(),
        column_widths: table.col_widths().to_vec(),
        borders_visible: table.borders_visible(),
        content: table
            .contents()
            .iter()
            .map(|row| {
                Ok(TableRow {
                    cells: row
                        .contents()
                        .iter()
                        .map(|cell| {
                            Ok(TableCell {
                                background: cell.background_color().map(map_color),
                                content: cell
                                    .contents()
                                    .iter()
                                    .map(|element| map_outline_element(element, policy, ink_policy))
                                    .collect::<Result<_>>()?,
                            })
                        })
                        .collect::<Result<_>>()?,
                })
            })
            .collect::<Result<_>>()?,
    })
}

fn map_image(image: &NativeImage, policy: BinaryDataPolicy) -> Result<Image> {
    let size = image.size().unwrap_or_default();
    let bytes = if policy.should_load(size) {
        image
            .read()
            .map(|reader| read_all(reader, "image"))
            .transpose()?
    } else {
        None
    };

    Ok(Image {
        filename: image.image_filename().map(str::to_owned),
        extension: image.extension().map(str::to_owned),
        alt_text: image.alt_text().map(str::to_owned),
        hyperlink: image.hyperlink_url().map(str::to_owned),
        background: image.is_background(),
        layout: Layout {
            x: image.offset_horizontal(),
            y: image.offset_vertical(),
            width: image.layout_max_width().or(image.picture_width()),
            height: image.layout_max_height().or(image.picture_height()),
        },
        blob: Blob::new(size, bytes),
    })
}

fn map_attachment(file: &EmbeddedFile, policy: BinaryDataPolicy) -> Result<Attachment> {
    let size = file.size();
    let bytes = if policy.should_load(size) {
        Some(read_all(file.read(), "attachment")?)
    } else {
        None
    };

    Ok(Attachment {
        filename: file.filename().to_owned(),
        layout: Layout {
            x: file.offset_horizontal(),
            y: file.offset_vertical(),
            width: file.layout_max_width(),
            height: file.layout_max_height(),
        },
        blob: Blob::new(size, bytes),
    })
}

fn read_all(mut reader: Box<dyn Read>, kind: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| Error::BinaryData(format!("{kind}: {error}")))?;
    Ok(bytes)
}

fn map_ink(ink: &NativeInk, policy: InkDataPolicy) -> Ink {
    Ink {
        layout: Layout {
            x: ink.offset_horizontal(),
            y: ink.offset_vertical(),
            width: ink.bounding_box().map(|bounds| bounds.width()),
            height: ink.bounding_box().map(|bounds| bounds.height()),
        },
        loaded: policy == InkDataPolicy::All,
        stroke_count: count_ink_strokes(ink),
        strokes: if policy == InkDataPolicy::All {
            ink.ink_strokes()
                .iter()
                .map(|stroke| InkStroke {
                    points: stroke
                        .path()
                        .iter()
                        .map(|point| Point {
                            x: point.x(),
                            y: point.y(),
                        })
                        .collect(),
                    pen_tip: stroke.pen_tip(),
                    transparency: stroke.transparency(),
                    width: stroke.width(),
                    height: stroke.height(),
                    color: stroke.color(),
                })
                .collect()
        } else {
            Vec::new()
        },
        groups: ink
            .child_groups()
            .iter()
            .map(|child| map_ink(child, policy))
            .collect(),
    }
}

fn count_ink_strokes(ink: &NativeInk) -> usize {
    ink.ink_strokes()
        .len()
        .saturating_add(ink.child_groups().iter().map(count_ink_strokes).sum())
}

fn collect_report(report: &Report, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.extend(report.warnings().iter().map(|warning| {
        let page = warning.page();
        Diagnostic {
            page_id: page.map(|(id, _)| id.to_string()),
            page_title: page.map(|(_, title)| title.to_owned()),
            message: warning.message().to_owned(),
        }
    }));
}

fn map_color(color: NativeColor) -> Color {
    Color {
        red: color.r(),
        green: color.g(),
        blue: color.b(),
        alpha: color.alpha(),
    }
}

fn map_color_ref(color: ColorRef) -> ColorReference {
    match color {
        ColorRef::Auto => ColorReference::Automatic,
        ColorRef::Manual { r, g, b } => ColorReference::Rgb {
            red: r,
            green: g,
            blue: b,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_extensions_case_insensitively() {
        assert_eq!(
            source_format(Path::new("Notebook.ONEPKG")).unwrap(),
            SourceFormat::Package
        );
        assert_eq!(
            source_format(Path::new("Section.one")).unwrap(),
            SourceFormat::Section
        );
    }

    #[test]
    fn binary_policy_honors_limits() {
        assert!(!BinaryDataPolicy::MetadataOnly.should_load(1));
        assert!(BinaryDataPolicy::UpTo(10).should_load(10));
        assert!(!BinaryDataPolicy::UpTo(10).should_load(11));
        assert!(BinaryDataPolicy::All.should_load(u64::MAX));
    }

    #[test]
    fn ink_is_metadata_only_by_default() {
        assert_eq!(LoadOptions::default().ink_data, InkDataPolicy::MetadataOnly);
    }
}
