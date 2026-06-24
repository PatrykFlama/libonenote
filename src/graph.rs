use crate::{
    Attachment, Blob, Color, ColorReference, Content, Document, Error, Image, Ink, Layout,
    Notebook, NotebookEntry, OutlineElement, OutlineItem, Page, PageBlock, Paragraph, Result,
    Table, TextAlignment, TextRun, TextStyle,
};
use serde::Serialize;
use std::fmt::Write as _;

const HALF_INCH_TO_PIXELS: f32 = 48.0;

/// Policy for content that Microsoft Graph cannot represent faithfully.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphUnsupportedPolicy {
    /// Stop before producing a lossy export.
    #[default]
    Reject,
    /// Omit the content and report a warning.
    Omit,
    /// Insert a visible textual placeholder and report a warning.
    Placeholder,
}

/// Options used when serializing the model for Microsoft Graph OneNote APIs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct GraphWriteOptions {
    /// How to handle native ink, which Graph cannot create as editable strokes.
    pub ink: GraphUnsupportedPolicy,
    /// How to handle unknown native OneNote objects.
    pub unknown: GraphUnsupportedPolicy,
}

impl GraphWriteOptions {
    /// Strict mode refuses any content that would be lost.
    pub const fn strict() -> Self {
        Self {
            ink: GraphUnsupportedPolicy::Reject,
            unknown: GraphUnsupportedPolicy::Reject,
        }
    }

    /// Best-effort mode preserves page structure and inserts visible placeholders.
    pub const fn with_placeholders() -> Self {
        Self {
            ink: GraphUnsupportedPolicy::Placeholder,
            unknown: GraphUnsupportedPolicy::Placeholder,
        }
    }
}

/// A notebook serialized into requests suitable for Microsoft Graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphNotebookExport {
    /// Notebook display name.
    pub notebook_name: String,
    /// Pages in section order.
    pub pages: Vec<GraphPageExport>,
}

/// A OneNote page represented as Graph input HTML and multipart resources.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphPageExport {
    /// Names of containing section groups followed by the section name.
    pub section_path: Vec<String>,
    /// Source page identifier when the page came from a native document.
    pub source_page_id: String,
    /// Page title.
    pub title: String,
    /// XHTML accepted by Graph create-page and update-page operations.
    pub html: String,
    /// Binary parts referenced by `name:<part_name>` URLs in the HTML.
    pub resources: Vec<GraphResource>,
    /// Explicit fidelity warnings produced during serialization.
    pub warnings: Vec<GraphWriteWarning>,
}

/// A binary multipart part referenced by a Graph page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphResource {
    /// Multipart part name.
    pub part_name: String,
    /// Original or generated filename.
    pub filename: String,
    /// MIME media type.
    pub content_type: String,
    /// Whether the part is displayed as an image or file attachment.
    pub kind: GraphResourceKind,
    #[serde(skip)]
    bytes: Vec<u8>,
}

impl GraphResource {
    /// Binary contents for the multipart request.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Graph multipart resource kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphResourceKind {
    /// Image displayed on the page.
    Image,
    /// Downloadable file attachment.
    Attachment,
}

/// A fidelity warning emitted by the Graph writer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GraphWriteWarning {
    /// Page containing the affected object.
    pub page: String,
    /// Human-readable explanation.
    pub message: String,
}

impl Document {
    /// Serialize the owned notebook model for Microsoft Graph OneNote APIs.
    ///
    /// This does not perform authentication or network requests. Binary images
    /// and attachments require loading with [`crate::BinaryDataPolicy::All`].
    pub fn to_graph_export(&self, options: GraphWriteOptions) -> Result<GraphNotebookExport> {
        self.notebook().to_graph_export(options)
    }
}

impl Notebook {
    /// Serialize this notebook into Graph-compatible page payloads.
    pub fn to_graph_export(&self, options: GraphWriteOptions) -> Result<GraphNotebookExport> {
        let mut pages = Vec::new();
        append_entries(&self.entries, &mut Vec::new(), options, &mut pages)?;
        Ok(GraphNotebookExport {
            notebook_name: self.name.clone(),
            pages,
        })
    }
}

fn append_entries(
    entries: &[NotebookEntry],
    group_path: &mut Vec<String>,
    options: GraphWriteOptions,
    output: &mut Vec<GraphPageExport>,
) -> Result<()> {
    for entry in entries {
        match entry {
            NotebookEntry::Section(section) => {
                group_path.push(section.name.clone());
                for page in &section.pages {
                    output.push(PageWriter::new(page, group_path.clone(), options).write()?);
                }
                group_path.pop();
            }
            NotebookEntry::SectionGroup(group) => {
                group_path.push(group.name.clone());
                append_entries(&group.entries, group_path, options, output)?;
                group_path.pop();
            }
        }
    }
    Ok(())
}

struct PageWriter<'a> {
    page: &'a Page,
    section_path: Vec<String>,
    options: GraphWriteOptions,
    resources: Vec<GraphResource>,
    warnings: Vec<GraphWriteWarning>,
    next_id: usize,
}

impl<'a> PageWriter<'a> {
    fn new(page: &'a Page, section_path: Vec<String>, options: GraphWriteOptions) -> Self {
        Self {
            page,
            section_path,
            options,
            resources: Vec::new(),
            warnings: Vec::new(),
            next_id: 0,
        }
    }

    fn write(mut self) -> Result<GraphPageExport> {
        let mut body = String::new();
        for block in &self.page.blocks {
            self.write_page_block(block, &mut body)?;
        }
        let title = escape_text(&self.page.title);
        let created = if self.page.created_at.is_empty() {
            String::new()
        } else {
            format!(
                "<meta name=\"created\" content=\"{}\" />",
                escape_attr(&self.page.created_at)
            )
        };
        let html = format!(
            "<!DOCTYPE html><html><head><title>{title}</title>{created}</head>\
             <body data-absolute-enabled=\"true\">{body}</body></html>"
        );
        Ok(GraphPageExport {
            section_path: self.section_path,
            source_page_id: self.page.id.clone(),
            title: self.page.title.clone(),
            html,
            resources: self.resources,
            warnings: self.warnings,
        })
    }

    fn write_page_block(&mut self, block: &PageBlock, output: &mut String) -> Result<()> {
        match block {
            PageBlock::Outline(outline) => {
                let id = self.element_id("outline");
                write!(
                    output,
                    "<div data-id=\"{id}\"{}>",
                    absolute_style(outline.layout, false)
                )
                .unwrap();
                for item in &outline.items {
                    self.write_outline_item(item, output)?;
                }
                output.push_str("</div>");
            }
            PageBlock::Image(image) => self.write_image(image, true, output)?,
            PageBlock::Attachment(file) => self.write_attachment(file, true, output)?,
            PageBlock::Ink(ink) => self.write_unsupported_ink(ink, true, output)?,
            PageBlock::Unknown => self.write_unknown(true, output)?,
        }
        Ok(())
    }

    fn write_outline_item(&mut self, item: &OutlineItem, output: &mut String) -> Result<()> {
        match item {
            OutlineItem::Element(element) => self.write_element(element, output),
            OutlineItem::Group(group) => {
                for item in &group.items {
                    self.write_outline_item(item, output)?;
                }
                Ok(())
            }
        }
    }

    fn write_element(&mut self, element: &OutlineElement, output: &mut String) -> Result<()> {
        for content in &element.content {
            match content {
                Content::Paragraph(paragraph) => write_paragraph(paragraph, output),
                Content::Table(table) => self.write_table(table, output)?,
                Content::Image(image) => self.write_image(image, false, output)?,
                Content::Attachment(file) => self.write_attachment(file, false, output)?,
                Content::Ink(ink) => self.write_unsupported_ink(ink, false, output)?,
                Content::Unknown => self.write_unknown(false, output)?,
            }
        }
        for child in &element.children {
            self.write_outline_item(child, output)?;
        }
        Ok(())
    }

    fn write_table(&mut self, table: &Table, output: &mut String) -> Result<()> {
        if table.borders_visible {
            output.push_str("<table border=\"1\">");
        } else {
            output.push_str("<table>");
        }
        for row in &table.content {
            output.push_str("<tr>");
            for cell in &row.cells {
                if let Some(background) = cell.background {
                    write!(
                        output,
                        "<td style=\"background-color:{}\">",
                        css_color(background)
                    )
                    .unwrap();
                } else {
                    output.push_str("<td>");
                }
                for element in &cell.content {
                    self.write_element(element, output)?;
                }
                output.push_str("</td>");
            }
            output.push_str("</tr>");
        }
        output.push_str("</table>");
        Ok(())
    }

    fn write_image(&mut self, image: &Image, positioned: bool, output: &mut String) -> Result<()> {
        let filename = image
            .filename
            .clone()
            .or_else(|| {
                image
                    .extension
                    .as_ref()
                    .map(|extension| format!("image.{extension}"))
            })
            .unwrap_or_else(|| "image.bin".to_owned());
        let part_name = self.add_resource(
            &filename,
            image.blob.clone(),
            GraphResourceKind::Image,
            image.extension.as_deref(),
        )?;
        let id = self.element_id("image");
        write!(
            output,
            "<img data-id=\"{id}\" src=\"name:{}\" alt=\"{}\"{} />",
            escape_attr(&part_name),
            escape_attr(image.alt_text.as_deref().unwrap_or("")),
            if positioned {
                absolute_style(image.layout, true)
            } else {
                size_attributes(image.layout)
            }
        )
        .unwrap();
        Ok(())
    }

    fn write_attachment(
        &mut self,
        file: &Attachment,
        positioned: bool,
        output: &mut String,
    ) -> Result<()> {
        let part_name = self.add_resource(
            &file.filename,
            file.blob.clone(),
            GraphResourceKind::Attachment,
            None,
        )?;
        let id = self.element_id("attachment");
        write!(
            output,
            "<object data-id=\"{id}\" data=\"name:{}\" data-attachment=\"{}\" type=\"{}\"{} />",
            escape_attr(&part_name),
            escape_attr(&file.filename),
            escape_attr(content_type(&file.filename, None)),
            if positioned {
                absolute_style(file.layout, false)
            } else {
                String::new()
            }
        )
        .unwrap();
        Ok(())
    }

    fn write_unsupported_ink(
        &mut self,
        ink: &Ink,
        positioned: bool,
        output: &mut String,
    ) -> Result<()> {
        match self.options.ink {
            GraphUnsupportedPolicy::Reject => Err(Error::GraphWriteUnsupported {
                page: self.page.title.clone(),
                content: "editable ink strokes",
            }),
            GraphUnsupportedPolicy::Omit => {
                self.warn(format!("omitted {} ink strokes", ink.stroke_count));
                Ok(())
            }
            GraphUnsupportedPolicy::Placeholder => {
                self.warn(format!(
                    "replaced {} editable ink strokes with a text placeholder",
                    ink.stroke_count
                ));
                if positioned {
                    write!(
                        output,
                        "<div{}><p>[Ink: {} strokes]</p></div>",
                        absolute_style(ink.layout, false),
                        ink.stroke_count
                    )
                    .unwrap();
                } else {
                    write!(output, "<p>[Ink: {} strokes]</p>", ink.stroke_count).unwrap();
                }
                Ok(())
            }
        }
    }

    fn write_unknown(&mut self, positioned: bool, output: &mut String) -> Result<()> {
        match self.options.unknown {
            GraphUnsupportedPolicy::Reject => Err(Error::GraphWriteUnsupported {
                page: self.page.title.clone(),
                content: "an unknown OneNote object",
            }),
            GraphUnsupportedPolicy::Omit => {
                self.warn("omitted an unknown OneNote object".to_owned());
                Ok(())
            }
            GraphUnsupportedPolicy::Placeholder => {
                self.warn("replaced an unknown OneNote object with a text placeholder".to_owned());
                if positioned {
                    output.push_str("<div><p>[Unsupported OneNote object]</p></div>");
                } else {
                    output.push_str("<p>[Unsupported OneNote object]</p>");
                }
                Ok(())
            }
        }
    }

    fn add_resource(
        &mut self,
        filename: &str,
        blob: Blob,
        kind: GraphResourceKind,
        extension: Option<&str>,
    ) -> Result<String> {
        let bytes = blob
            .bytes
            .ok_or_else(|| Error::MissingBinaryData(filename.to_owned()))?;
        let part_name = format!("resource-{}", self.resources.len());
        self.resources.push(GraphResource {
            part_name: part_name.clone(),
            filename: filename.to_owned(),
            content_type: content_type(filename, extension).to_owned(),
            kind,
            bytes,
        });
        Ok(part_name)
    }

    fn element_id(&mut self, kind: &str) -> String {
        let id = format!("{kind}-{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn warn(&mut self, message: String) {
        self.warnings.push(GraphWriteWarning {
            page: self.page.title.clone(),
            message,
        });
    }
}

fn write_paragraph(paragraph: &Paragraph, output: &mut String) {
    let mut style = Vec::new();
    match paragraph.alignment {
        TextAlignment::Center => style.push("text-align:center".to_owned()),
        TextAlignment::Right => style.push("text-align:right".to_owned()),
        TextAlignment::Left | TextAlignment::Unknown => {}
    }
    if paragraph.space_before > 0.0 {
        style.push(format!("margin-top:{}pt", paragraph.space_before));
    }
    if paragraph.space_after > 0.0 {
        style.push(format!("margin-bottom:{}pt", paragraph.space_after));
    }
    if style.is_empty() {
        output.push_str("<p>");
    } else {
        write!(output, "<p style=\"{}\">", style.join(";")).unwrap();
    }
    if paragraph.runs.is_empty() {
        write_styled_text(&paragraph.text, &paragraph.style, output);
    } else {
        for run in &paragraph.runs {
            write_run(run, output);
        }
    }
    output.push_str("</p>");
}

fn write_run(run: &TextRun, output: &mut String) {
    write_styled_text(&run.text, &run.style, output);
}

fn write_styled_text(text: &str, style: &TextStyle, output: &mut String) {
    if style.hidden {
        return;
    }
    let mut css = Vec::new();
    if style.bold {
        css.push("font-weight:bold".to_owned());
    }
    if style.italic {
        css.push("font-style:italic".to_owned());
    }
    if style.underline {
        css.push("text-decoration:underline".to_owned());
    } else if style.strikethrough {
        css.push("text-decoration:line-through".to_owned());
    }
    if let Some(font) = &style.font {
        css.push(format!("font-family:{}", escape_css(font)));
    }
    if let Some(size) = style.font_size_half_points {
        css.push(format!("font-size:{}pt", size as f32 * 0.5));
    }
    if let Some(ColorReference::Rgb { red, green, blue }) = style.color {
        css.push(format!("color:#{red:02x}{green:02x}{blue:02x}"));
    }
    if let Some(ColorReference::Rgb { red, green, blue }) = style.highlight {
        css.push(format!("background-color:#{red:02x}{green:02x}{blue:02x}"));
    }

    if style.superscript {
        output.push_str("<sup>");
    } else if style.subscript {
        output.push_str("<sub>");
    }
    if css.is_empty() {
        output.push_str(&escape_text(text));
    } else {
        write!(
            output,
            "<span style=\"{}\">{}</span>",
            css.join(";"),
            escape_text(text)
        )
        .unwrap();
    }
    if style.superscript {
        output.push_str("</sup>");
    } else if style.subscript {
        output.push_str("</sub>");
    }
}

fn absolute_style(layout: Layout, include_height: bool) -> String {
    let mut style = vec!["position:absolute".to_owned()];
    if let Some(x) = layout.x {
        style.push(format!("left:{}px", x * HALF_INCH_TO_PIXELS));
    }
    if let Some(y) = layout.y {
        style.push(format!("top:{}px", y * HALF_INCH_TO_PIXELS));
    }
    if let Some(width) = layout.width {
        style.push(format!("width:{}px", width * HALF_INCH_TO_PIXELS));
    }
    if include_height && let Some(height) = layout.height {
        style.push(format!("height:{}px", height * HALF_INCH_TO_PIXELS));
    }
    format!(" style=\"{}\"", style.join(";"))
}

fn size_attributes(layout: Layout) -> String {
    let width = layout
        .width
        .map(|value| format!(" width=\"{}\"", value * HALF_INCH_TO_PIXELS))
        .unwrap_or_default();
    let height = layout
        .height
        .map(|value| format!(" height=\"{}\"", value * HALF_INCH_TO_PIXELS))
        .unwrap_or_default();
    format!("{width}{height}")
}

fn css_color(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
}

fn content_type<'a>(filename: &'a str, extension: Option<&'a str>) -> &'a str {
    let extension = extension
        .or_else(|| filename.rsplit_once('.').map(|(_, extension)| extension))
        .unwrap_or_default();
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_css(value: &str) -> String {
    value.replace([';', '"', '\'', '<', '>'], "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Section, Source, SourceFormat};

    fn paragraph(text: &str) -> Paragraph {
        Paragraph {
            text: text.to_owned(),
            style: TextStyle::default(),
            runs: Vec::new(),
            alignment: TextAlignment::Left,
            space_before: 0.0,
            space_after: 0.0,
        }
    }

    fn page(blocks: Vec<PageBlock>) -> Page {
        Page {
            id: "page-id".to_owned(),
            title: "A & B".to_owned(),
            level: 0,
            author: None,
            created_at: String::new(),
            updated_at: String::new(),
            height: None,
            blocks,
        }
    }

    fn document(page: Page) -> Document {
        Document::loaded(
            Source {
                path: None,
                format: SourceFormat::InMemory,
            },
            Notebook {
                name: "Notebook".to_owned(),
                color: None,
                entries: vec![NotebookEntry::Section(Section {
                    name: "Section".to_owned(),
                    color: None,
                    pages: vec![page],
                })],
            },
            Vec::new(),
            None,
            crate::LoadOptions::default(),
        )
    }

    #[test]
    fn serializes_positioned_text_as_graph_html() {
        let document = document(page(vec![PageBlock::Outline(crate::Outline {
            level: 0,
            layout: Layout {
                x: Some(2.0),
                y: Some(3.0),
                width: Some(4.0),
                height: None,
            },
            items: vec![OutlineItem::Element(OutlineElement {
                level: 0,
                lists: Vec::new(),
                content: vec![Content::Paragraph(paragraph("<hello>"))],
                children: Vec::new(),
            })],
        })]));

        let export = document
            .to_graph_export(GraphWriteOptions::strict())
            .unwrap();

        assert_eq!(export.pages[0].section_path, ["Section"]);
        assert!(export.pages[0].html.contains("<title>A &amp; B</title>"));
        assert!(export.pages[0].html.contains("left:96px;top:144px"));
        assert!(export.pages[0].html.contains("&lt;hello&gt;"));
    }

    #[test]
    fn strict_graph_write_rejects_ink() {
        let document = document(page(vec![PageBlock::Ink(Ink {
            layout: Layout::default(),
            loaded: true,
            stroke_count: 2,
            strokes: Vec::new(),
            groups: Vec::new(),
        })]));

        assert!(matches!(
            document.to_graph_export(GraphWriteOptions::strict()),
            Err(Error::GraphWriteUnsupported {
                content: "editable ink strokes",
                ..
            })
        ));
    }

    #[test]
    fn placeholder_mode_reports_ink_loss() {
        let document = document(page(vec![PageBlock::Ink(Ink {
            layout: Layout::default(),
            loaded: true,
            stroke_count: 2,
            strokes: Vec::new(),
            groups: Vec::new(),
        })]));

        let export = document
            .to_graph_export(GraphWriteOptions::with_placeholders())
            .unwrap();

        assert!(export.pages[0].html.contains("[Ink: 2 strokes]"));
        assert_eq!(export.pages[0].warnings.len(), 1);
    }

    #[test]
    fn image_requires_loaded_binary_data() {
        let document = document(page(vec![PageBlock::Image(Image {
            filename: Some("image.png".to_owned()),
            extension: Some("png".to_owned()),
            alt_text: None,
            hyperlink: None,
            background: false,
            layout: Layout::default(),
            blob: Blob::new(10, None),
        })]));

        assert!(matches!(
            document.to_graph_export(GraphWriteOptions::strict()),
            Err(Error::MissingBinaryData(filename)) if filename == "image.png"
        ));
    }
}
