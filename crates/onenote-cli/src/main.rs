use clap::{Parser, Subcommand};
use libonenote::{
    BinaryDataPolicy, Content, Document, GraphWriteOptions, Loader, NotebookEntry, OutlineElement,
    OutlineItem, PageBlock, Result,
};
use sanitize_filename::{Options, sanitize_with_options};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "onenote", version, about = "Inspect Microsoft OneNote files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print notebook structure and capabilities.
    Info {
        /// OneNote input file.
        input: PathBuf,
    },
    /// Print the high-level document model as JSON.
    Json {
        /// OneNote input file.
        input: PathBuf,
        /// Emit compact JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Extract images and attachments.
    Extract {
        /// OneNote input file.
        input: PathBuf,
        /// Output directory.
        output: PathBuf,
    },
    /// Make an exact byte-for-byte copy of an unchanged .one or .onepkg file.
    Copy {
        /// OneNote input file.
        input: PathBuf,
        /// Output native file.
        output: PathBuf,
    },
    /// Rename one page in a .one section or .onepkg and save a verified copy.
    ///
    /// The replacement must fit in the existing native property allocation.
    RenamePage {
        /// Input .one section or .onepkg package.
        input: PathBuf,
        /// Output file with the same native container type.
        output: PathBuf,
        /// Exact current page title.
        old_title: String,
        /// Replacement page title.
        new_title: String,
    },
    /// Replace one complete paragraph in a .one or .onepkg and save a verified copy.
    ///
    /// The replacement must fit in the existing native property allocation.
    ReplaceText {
        /// Input .one section or .onepkg package.
        input: PathBuf,
        /// Output file with the same native container type.
        output: PathBuf,
        /// Exact current paragraph text.
        old_text: String,
        /// Replacement paragraph text.
        new_text: String,
    },
    /// Write Microsoft Graph page XHTML and multipart resources.
    GraphExport {
        /// OneNote input file.
        input: PathBuf,
        /// Output directory.
        output: PathBuf,
        /// Replace unsupported ink and unknown objects with visible placeholders.
        #[arg(long)]
        placeholders: bool,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Info { input } => print_info(Loader::new().open(input)?),
        Command::Json { input, compact } => {
            let document = Loader::new().open(input)?;
            println!("{}", document.to_json(!compact)?);
            Ok(())
        }
        Command::Extract { input, output } => {
            let document = Loader::new()
                .binary_data(BinaryDataPolicy::All)
                .open(input)?;
            extract(&document, &output)
        }
        Command::Copy { input, output } => Loader::new()
            .preserve_original(true)
            .open(input)?
            .save_native(output),
        Command::RenamePage {
            input,
            output,
            old_title,
            new_title,
        } => rename_page(&input, &output, &old_title, &new_title),
        Command::ReplaceText {
            input,
            output,
            old_text,
            new_text,
        } => replace_text(&input, &output, &old_text, &new_text),
        Command::GraphExport {
            input,
            output,
            placeholders,
        } => {
            let document = Loader::new()
                .binary_data(BinaryDataPolicy::All)
                .open(input)?;
            graph_export(&document, &output, placeholders)
        }
    }
}

fn rename_page(input: &Path, output: &Path, old_title: &str, new_title: &str) -> Result<()> {
    let mut document = Loader::new().preserve_original(true).open(input)?;
    document.edit(|notebook| {
        let mut matches = notebook
            .entries
            .iter_mut()
            .filter_map(|entry| match entry {
                NotebookEntry::Section(section) => Some(section),
                NotebookEntry::SectionGroup(_) => None,
            })
            .flat_map(|section| &mut section.pages)
            .filter(|page| page.title == old_title);
        let page = matches
            .next()
            .ok_or_else(|| libonenote::Error::NativeTextNotFound(old_title.to_owned()))?;
        if matches.next().is_some() {
            return Err(libonenote::Error::NativeWriteUnsupported);
        }
        page.title = new_title.to_owned();
        Ok(())
    })?;
    document.save_native(output)
}

fn replace_text(input: &Path, output: &Path, old_text: &str, new_text: &str) -> Result<()> {
    let mut document = Loader::new().preserve_original(true).open(input)?;
    document.edit(|notebook| {
        let mut replacements = 0;
        for entry in &mut notebook.entries {
            replacements += replace_entry_text(entry, old_text, new_text)?;
        }
        match replacements {
            0 => Err(libonenote::Error::NativeTextNotFound(old_text.to_owned())),
            1 => Ok(()),
            _ => Err(libonenote::Error::NativeWriteUnsupported),
        }
    })?;
    document.save_native(output)
}

fn replace_entry_text(entry: &mut NotebookEntry, old: &str, new: &str) -> Result<usize> {
    match entry {
        NotebookEntry::Section(section) => {
            let mut replacements = 0;
            for page in &mut section.pages {
                for block in &mut page.blocks {
                    if let PageBlock::Outline(outline) = block {
                        for item in &mut outline.items {
                            replacements += replace_outline_item_text(item, old, new)?;
                        }
                    }
                }
            }
            Ok(replacements)
        }
        NotebookEntry::SectionGroup(group) => {
            let mut replacements = 0;
            for entry in &mut group.entries {
                replacements += replace_entry_text(entry, old, new)?;
            }
            Ok(replacements)
        }
    }
}

fn replace_outline_item_text(item: &mut OutlineItem, old: &str, new: &str) -> Result<usize> {
    match item {
        OutlineItem::Element(element) => replace_element_text(element, old, new),
        OutlineItem::Group(group) => {
            let mut replacements = 0;
            for item in &mut group.items {
                replacements += replace_outline_item_text(item, old, new)?;
            }
            Ok(replacements)
        }
    }
}

fn replace_element_text(element: &mut OutlineElement, old: &str, new: &str) -> Result<usize> {
    let mut replacements = 0;
    for content in &mut element.content {
        match content {
            Content::Paragraph(paragraph) if paragraph.text == old => {
                replace_paragraph_text(paragraph, new)?;
                replacements += 1;
            }
            Content::Table(table) => {
                for row in &mut table.content {
                    for cell in &mut row.cells {
                        for element in &mut cell.content {
                            replacements += replace_element_text(element, old, new)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for child in &mut element.children {
        replacements += replace_outline_item_text(child, old, new)?;
    }
    Ok(replacements)
}

fn replace_paragraph_text(paragraph: &mut libonenote::Paragraph, new: &str) -> Result<()> {
    let old_units = paragraph.text.encode_utf16().count();
    let new_units = new.encode_utf16().collect::<Vec<_>>();
    if new_units.len() > old_units {
        return Err(libonenote::Error::NativeWriteSizeChangeUnsupported);
    }
    if !paragraph.runs.is_empty()
        && paragraph
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>()
            != paragraph.text
    {
        return Err(libonenote::Error::NativeWriteUnsupported);
    }

    let fixed_run_units = paragraph
        .runs
        .iter()
        .take(paragraph.runs.len().saturating_sub(1))
        .map(|run| run.text.encode_utf16().count())
        .sum::<usize>();
    if new_units.len() < fixed_run_units {
        return Err(libonenote::Error::NativeWriteUnsupported);
    }

    let run_count = paragraph.runs.len();
    let mut offset = 0;
    for (index, run) in paragraph.runs.iter_mut().enumerate() {
        let length = if index + 1 == run_count {
            new_units.len() - offset
        } else {
            run.text.encode_utf16().count()
        };
        run.text = String::from_utf16(&new_units[offset..offset + length])
            .map_err(|_| libonenote::Error::NativeWriteUnsupported)?;
        offset += length;
    }
    paragraph.text = new.to_owned();
    Ok(())
}

fn print_info(document: Document) -> Result<()> {
    let notebook = document.notebook();
    println!("Notebook: {}", notebook.name);
    println!("Format: {:?}", document.source().format);
    println!("Sections: {}", notebook.sections().count());
    println!("Pages: {}", notebook.pages().count());
    println!("Diagnostics: {}", document.diagnostics().len());
    println!("Capabilities: {:?}", document.capabilities());

    for entry in &notebook.entries {
        print_entry(entry, 0);
    }
    Ok(())
}

fn print_entry(entry: &NotebookEntry, depth: usize) {
    let indent = "  ".repeat(depth);
    match entry {
        NotebookEntry::Section(section) => {
            println!(
                "{indent}Section: {} ({} pages)",
                section.name,
                section.pages.len()
            );
            for page in &section.pages {
                println!("{indent}  Page: {}", page.title);
            }
        }
        NotebookEntry::SectionGroup(group) => {
            println!("{indent}Group: {}", group.name);
            for child in &group.entries {
                print_entry(child, depth + 1);
            }
        }
    }
}

fn extract(document: &Document, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut index = 0usize;

    for page in document.notebook().pages() {
        for block in &page.blocks {
            extract_page_block(block, output, &mut index)?;
        }
    }

    println!("Extracted {index} binary objects to {}", output.display());
    Ok(())
}

fn extract_page_block(block: &PageBlock, output: &Path, index: &mut usize) -> Result<()> {
    match block {
        PageBlock::Outline(outline) => {
            for item in &outline.items {
                extract_outline_item(item, output, index)?;
            }
        }
        PageBlock::Image(image) => {
            write_blob(
                image.filename.as_deref().unwrap_or("image.bin"),
                image.blob.bytes(),
                output,
                index,
            )?;
        }
        PageBlock::Attachment(file) => {
            write_blob(&file.filename, file.blob.bytes(), output, index)?;
        }
        PageBlock::Ink(_) | PageBlock::Unknown => {}
    }
    Ok(())
}

fn extract_outline_item(item: &OutlineItem, output: &Path, index: &mut usize) -> Result<()> {
    match item {
        OutlineItem::Element(element) => extract_element(element, output, index),
        OutlineItem::Group(group) => {
            for item in &group.items {
                extract_outline_item(item, output, index)?;
            }
            Ok(())
        }
    }
}

fn extract_element(element: &OutlineElement, output: &Path, index: &mut usize) -> Result<()> {
    for content in &element.content {
        match content {
            Content::Image(image) => write_blob(
                image.filename.as_deref().unwrap_or("image.bin"),
                image.blob.bytes(),
                output,
                index,
            )?,
            Content::Attachment(file) => {
                write_blob(&file.filename, file.blob.bytes(), output, index)?
            }
            Content::Table(table) => {
                for row in &table.content {
                    for cell in &row.cells {
                        for element in &cell.content {
                            extract_element(element, output, index)?;
                        }
                    }
                }
            }
            Content::Paragraph(_) | Content::Ink(_) | Content::Unknown => {}
        }
    }
    for child in &element.children {
        extract_outline_item(child, output, index)?;
    }
    Ok(())
}

fn write_blob(
    original_name: &str,
    bytes: Option<&[u8]>,
    output: &Path,
    index: &mut usize,
) -> Result<()> {
    let Some(bytes) = bytes else {
        return Ok(());
    };
    *index += 1;
    let safe_name = sanitize_with_options(
        original_name,
        Options {
            windows: false,
            ..Options::default()
        },
    );
    let safe_name = if safe_name.is_empty() {
        "attachment.bin"
    } else {
        safe_name.as_str()
    };
    fs::write(output.join(format!("{index:04}-{safe_name}")), bytes)?;
    Ok(())
}

fn graph_export(document: &Document, output: &Path, placeholders: bool) -> Result<()> {
    let options = if placeholders {
        GraphWriteOptions::with_placeholders()
    } else {
        GraphWriteOptions::strict()
    };
    let export = document.to_graph_export(options)?;
    fs::create_dir_all(output)?;

    for (index, page) in export.pages.iter().enumerate() {
        let page_name = sanitize_with_options(
            &page.title,
            Options {
                windows: false,
                ..Options::default()
            },
        );
        let page_name = if page_name.is_empty() {
            "page"
        } else {
            page_name.as_str()
        };
        let page_directory = output.join(format!("{:04}-{page_name}", index + 1));
        fs::create_dir_all(&page_directory)?;
        fs::write(page_directory.join("page.xhtml"), &page.html)?;

        for resource in &page.resources {
            let filename = sanitize_with_options(
                &resource.filename,
                Options {
                    windows: false,
                    ..Options::default()
                },
            );
            let filename = if filename.is_empty() {
                "resource.bin"
            } else {
                filename.as_str()
            };
            fs::write(
                page_directory.join(format!("{}-{filename}", resource.part_name)),
                resource.bytes(),
            )?;
        }

        if !page.warnings.is_empty() {
            let warnings = page
                .warnings
                .iter()
                .map(|warning| warning.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(page_directory.join("warnings.txt"), warnings)?;
        }
    }

    println!(
        "Wrote {} Graph page payload(s) to {}",
        export.pages.len(),
        output.display()
    );
    Ok(())
}
