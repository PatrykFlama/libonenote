use crate::model::{
    Content, Notebook, NotebookEntry, OutlineElement, OutlineItem, Page, PageBlock,
};
use crate::{Error, Result, SourceFormat};

mod package;

pub(crate) fn write_modified_section(
    original: &[u8],
    baseline: &Notebook,
    edited: &Notebook,
) -> Result<Vec<u8>> {
    let edits = text_edits(baseline, edited)?;
    if edits.is_empty() {
        return Ok(original.to_vec());
    }

    let mut output = original.to_vec();
    for (old, new) in edits {
        if count_visible_text(baseline, &old) != 1 {
            return Err(Error::NativeWriteUnsupported);
        }
        patch_text(&mut output, &old, &new)?;
    }
    Ok(output)
}

pub(crate) fn verify_modified_section(expected: &Notebook, actual: &Notebook) -> Result<()> {
    let expected_pages = section_pages(expected)?;
    let actual_pages = section_pages(actual)?;
    if expected_pages == actual_pages {
        Ok(())
    } else {
        Err(Error::NativeWriteVerificationFailed)
    }
}

pub(crate) fn write_modified_package(
    original: &[u8],
    baseline: &Notebook,
    edited: &Notebook,
    options: crate::LoadOptions,
) -> Result<Vec<u8>> {
    package::write_modified_package(original, baseline, edited, options)
}

pub(crate) fn verify_modified_package(expected: &Notebook, actual: &Notebook) -> Result<()> {
    if expected.color == actual.color && expected.entries == actual.entries {
        Ok(())
    } else {
        Err(Error::NativeWriteVerificationFailed)
    }
}

fn text_edits(baseline: &Notebook, edited: &Notebook) -> Result<Vec<(String, String)>> {
    let (baseline_section, edited_section) = section_pair(baseline, edited)?;
    let baseline_pages = &baseline_section.pages;
    let mut normalized_pages = edited_section.pages.clone();
    if baseline_pages.len() != normalized_pages.len() {
        return Err(Error::NativeWriteUnsupported);
    }

    let mut edits = Vec::new();
    for (before, after) in baseline_pages.iter().zip(&mut normalized_pages) {
        if before.title != after.title {
            edits.push((before.title.clone(), after.title.clone()));
            after.title.clone_from(&before.title);
        }
        normalize_page_text(before, after, &mut edits);
    }
    if baseline_pages.as_slice() != normalized_pages {
        return Err(Error::NativeWriteUnsupported);
    }
    Ok(edits)
}

fn section_pair<'a>(
    baseline: &'a Notebook,
    edited: &'a Notebook,
) -> Result<(&'a crate::Section, &'a crate::Section)> {
    if baseline.name != edited.name || baseline.color != edited.color {
        return Err(Error::NativeWriteUnsupported);
    }
    match (baseline.entries.as_slice(), edited.entries.as_slice()) {
        ([NotebookEntry::Section(before)], [NotebookEntry::Section(after)])
            if before.name == after.name && before.color == after.color =>
        {
            Ok((before, after))
        }
        _ => Err(Error::NativeWriteUnsupported),
    }
}

fn section_pages(notebook: &Notebook) -> Result<&[Page]> {
    match notebook.entries.as_slice() {
        [NotebookEntry::Section(section)] => Ok(&section.pages),
        _ => Err(Error::NativeWriteUnsupported),
    }
}

fn normalize_page_text(before: &Page, after: &mut Page, edits: &mut Vec<(String, String)>) {
    for (before, after) in before.blocks.iter().zip(&mut after.blocks) {
        normalize_block_text(before, after, edits);
    }
}

fn normalize_block_text(
    before: &PageBlock,
    after: &mut PageBlock,
    edits: &mut Vec<(String, String)>,
) {
    if let (PageBlock::Outline(before), PageBlock::Outline(after)) = (before, after) {
        for (before, after) in before.items.iter().zip(&mut after.items) {
            normalize_outline_item_text(before, after, edits);
        }
    }
}

fn normalize_outline_item_text(
    before: &OutlineItem,
    after: &mut OutlineItem,
    edits: &mut Vec<(String, String)>,
) {
    match (before, after) {
        (OutlineItem::Element(before), OutlineItem::Element(after)) => {
            normalize_element_text(before, after, edits);
        }
        (OutlineItem::Group(before), OutlineItem::Group(after)) => {
            for (before, after) in before.items.iter().zip(&mut after.items) {
                normalize_outline_item_text(before, after, edits);
            }
        }
        _ => {}
    }
}

fn normalize_element_text(
    before: &OutlineElement,
    after: &mut OutlineElement,
    edits: &mut Vec<(String, String)>,
) {
    for (before, after) in before.content.iter().zip(&mut after.content) {
        match (before, after) {
            (Content::Paragraph(before), Content::Paragraph(after))
                if before.text != after.text =>
            {
                edits.push((before.text.clone(), after.text.clone()));
                after.text.clone_from(&before.text);
                for (before, after) in before.runs.iter().zip(&mut after.runs) {
                    after.text.clone_from(&before.text);
                }
            }
            (Content::Table(before), Content::Table(after)) => {
                for (before, after) in before.content.iter().zip(&mut after.content) {
                    for (before, after) in before.cells.iter().zip(&mut after.cells) {
                        for (before, after) in before.content.iter().zip(&mut after.content) {
                            normalize_element_text(before, after, edits);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (before, after) in before.children.iter().zip(&mut after.children) {
        normalize_outline_item_text(before, after, edits);
    }
}

fn patch_text(bytes: &mut [u8], old: &str, new: &str) -> Result<()> {
    let old_utf16 = encode_utf16_property(old);
    let mut new_utf16 = encode_utf16_property(new);
    if new_utf16.len() > old_utf16.len() {
        return Err(Error::NativeWriteSizeChangeUnsupported);
    }
    new_utf16.resize(old_utf16.len(), 0);

    let mut replacements = replace_length_prefixed_values(bytes, &old_utf16, &new_utf16);
    if let Some(old_latin1) = encode_latin1(old) {
        let latin1_values = count_length_prefixed_values(bytes, &old_latin1);
        if latin1_values > 0 {
            let Some(new_latin1) = encode_latin1(new) else {
                return Err(Error::NativeWriteEncodingChangeUnsupported);
            };
            if old_latin1.len() != new_latin1.len() {
                return Err(Error::NativeWriteSizeChangeUnsupported);
            }
            replacements += replace_length_prefixed_values(bytes, &old_latin1, &new_latin1);
        }
    }

    if replacements == 0 {
        Err(Error::NativeTextNotFound(old.to_owned()))
    } else {
        Ok(())
    }
}

fn encode_utf16_property(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

fn encode_latin1(value: &str) -> Option<Vec<u8>> {
    value
        .chars()
        .map(|character| u8::try_from(character as u32).ok())
        .collect()
}

fn replace_length_prefixed_values(haystack: &mut [u8], needle: &[u8], replacement: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut replacements = 0;
    let mut offset = 0;
    while let Some(found) = find_bytes(&haystack[offset..], needle) {
        let start = offset + found;
        if start >= 4
            && u32::from_le_bytes(haystack[start - 4..start].try_into().expect("four bytes"))
                == needle.len() as u32
        {
            haystack[start..start + needle.len()].copy_from_slice(replacement);
            replacements += 1;
        }
        offset = start + needle.len();
    }
    replacements
}

fn count_length_prefixed_values(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut matches = 0;
    let mut offset = 0;
    while let Some(found) = find_bytes(&haystack[offset..], needle) {
        let start = offset + found;
        if start >= 4
            && u32::from_le_bytes(haystack[start - 4..start].try_into().expect("four bytes"))
                == needle.len() as u32
        {
            matches += 1;
        }
        offset = start + needle.len();
    }
    matches
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|candidate| candidate == needle)
}

fn count_visible_text(notebook: &Notebook, text: &str) -> usize {
    notebook
        .pages()
        .map(|page| {
            page.title.matches(text).count()
                + page
                    .blocks
                    .iter()
                    .map(|block| count_block_text(block, text))
                    .sum::<usize>()
        })
        .sum()
}

fn count_block_text(block: &PageBlock, text: &str) -> usize {
    match block {
        PageBlock::Outline(outline) => outline
            .items
            .iter()
            .map(|item| count_outline_item_text(item, text))
            .sum(),
        PageBlock::Image(image) => image
            .alt_text
            .as_deref()
            .map_or(0, |alt| alt.matches(text).count()),
        PageBlock::Attachment(_) | PageBlock::Ink(_) | PageBlock::Unknown => 0,
    }
}

fn count_outline_item_text(item: &OutlineItem, text: &str) -> usize {
    match item {
        OutlineItem::Element(element) => count_element_text(element, text),
        OutlineItem::Group(group) => group
            .items
            .iter()
            .map(|item| count_outline_item_text(item, text))
            .sum(),
    }
}

fn count_element_text(element: &OutlineElement, text: &str) -> usize {
    let content = element
        .content
        .iter()
        .map(|content| match content {
            Content::Paragraph(paragraph) => paragraph.text.matches(text).count(),
            Content::Table(table) => table
                .content
                .iter()
                .flat_map(|row| &row.cells)
                .flat_map(|cell| &cell.content)
                .map(|element| count_element_text(element, text))
                .sum(),
            Content::Image(image) => image
                .alt_text
                .as_deref()
                .map_or(0, |alt| alt.matches(text).count()),
            Content::Attachment(_) | Content::Ink(_) | Content::Unknown => 0,
        })
        .sum::<usize>();
    content
        + element
            .children
            .iter()
            .map(|item| count_outline_item_text(item, text))
            .sum::<usize>()
}

pub(crate) fn supports_modified_native(format: SourceFormat) -> bool {
    matches!(format, SourceFormat::Section | SourceFormat::Package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Layout, Notebook, Outline, OutlineElement, OutlineItem, Paragraph, Section, TextAlignment,
        TextRun, TextStyle,
    };

    fn notebook(title: &str) -> Notebook {
        Notebook {
            name: "Test".to_owned(),
            color: None,
            entries: vec![NotebookEntry::Section(Section {
                name: "Section".to_owned(),
                color: None,
                pages: vec![Page {
                    id: "page".to_owned(),
                    title: title.to_owned(),
                    level: 0,
                    author: None,
                    created_at: String::new(),
                    updated_at: String::new(),
                    height: None,
                    blocks: Vec::new(),
                }],
            })],
        }
    }

    fn notebook_with_paragraph(text: &str) -> Notebook {
        let mut notebook = notebook("Page");
        let NotebookEntry::Section(section) = &mut notebook.entries[0] else {
            unreachable!()
        };
        section.pages[0].blocks = vec![PageBlock::Outline(Outline {
            level: 1,
            layout: Layout::default(),
            items: vec![OutlineItem::Element(OutlineElement {
                level: 1,
                lists: Vec::new(),
                content: vec![Content::Paragraph(Paragraph {
                    text: text.to_owned(),
                    style: TextStyle::default(),
                    runs: vec![TextRun {
                        text: text.to_owned(),
                        style: TextStyle::default(),
                    }],
                    alignment: TextAlignment::Left,
                    space_before: 0.0,
                    space_after: 0.0,
                })],
                children: Vec::new(),
            })],
        })];
        notebook
    }

    #[test]
    fn patches_latin1_and_utf16_title_copies_without_changing_file_size() {
        let baseline = notebook("Old title");
        let edited = notebook("New title");
        let old_latin1 = b"Old title";
        let old_utf16 = encode_utf16_property("Old title");
        let mut bytes = Vec::new();
        bytes.extend((old_latin1.len() as u32).to_le_bytes());
        bytes.extend(old_latin1);
        bytes.extend((old_utf16.len() as u32).to_le_bytes());
        bytes.extend(&old_utf16);
        let output = write_modified_section(&bytes, &baseline, &edited).unwrap();

        assert_eq!(output.len(), bytes.len());
        assert!(find_bytes(&output, b"New title").is_some());
        assert!(find_bytes(&output, &encode_utf16_property("New title")).is_some());
        assert!(find_bytes(&output, b"Old title").is_none());
    }

    #[test]
    fn does_not_patch_unframed_matching_bytes() {
        let baseline = notebook("Old");
        let edited = notebook("New");
        assert!(matches!(
            write_modified_section(b"unframed Old bytes", &baseline, &edited),
            Err(Error::NativeTextNotFound(title)) if title == "Old"
        ));
    }

    #[test]
    fn patches_same_size_paragraph_text() {
        let baseline = notebook_with_paragraph("Code");
        let edited = notebook_with_paragraph("Data");
        let old_utf16 = encode_utf16_property("Code");
        let mut bytes = Vec::new();
        bytes.extend((old_utf16.len() as u32).to_le_bytes());
        bytes.extend(old_utf16);

        let output = write_modified_section(&bytes, &baseline, &edited).unwrap();
        assert!(
            find_bytes(&output, &encode_utf16_property("Data")).is_some(),
            "paragraph property should be replaced"
        );
    }

    #[test]
    fn pads_shorter_utf16_text_inside_the_existing_property_allocation() {
        let baseline = notebook_with_paragraph("Zażółć");
        let mut edited = notebook_with_paragraph("Żółć");
        let NotebookEntry::Section(section) = &mut edited.entries[0] else {
            unreachable!()
        };
        let PageBlock::Outline(outline) = &mut section.pages[0].blocks[0] else {
            unreachable!()
        };
        let OutlineItem::Element(element) = &mut outline.items[0] else {
            unreachable!()
        };
        let Content::Paragraph(paragraph) = &mut element.content[0] else {
            unreachable!()
        };
        paragraph.runs[0].text = "Żółć".to_owned();

        let old_utf16 = encode_utf16_property("Zażółć");
        let mut bytes = Vec::new();
        bytes.extend((old_utf16.len() as u32).to_le_bytes());
        bytes.extend(old_utf16);
        let output = write_modified_section(&bytes, &baseline, &edited).unwrap();

        let mut padded = encode_utf16_property("Żółć");
        padded.resize(encode_utf16_property("Zażółć").len(), 0);
        assert!(find_bytes(&output, &padded).is_some());
    }

    #[test]
    fn rejects_shorter_text_when_a_single_byte_property_copy_exists() {
        let baseline = notebook_with_paragraph("Code");
        let mut edited = notebook_with_paragraph("C");
        let NotebookEntry::Section(section) = &mut edited.entries[0] else {
            unreachable!()
        };
        let PageBlock::Outline(outline) = &mut section.pages[0].blocks[0] else {
            unreachable!()
        };
        let OutlineItem::Element(element) = &mut outline.items[0] else {
            unreachable!()
        };
        let Content::Paragraph(paragraph) = &mut element.content[0] else {
            unreachable!()
        };
        paragraph.runs[0].text = "C".to_owned();

        let old_utf16 = encode_utf16_property("Code");
        let mut bytes = Vec::new();
        bytes.extend((old_utf16.len() as u32).to_le_bytes());
        bytes.extend(old_utf16);
        bytes.extend(4_u32.to_le_bytes());
        bytes.extend(b"Code");

        assert!(matches!(
            write_modified_section(&bytes, &baseline, &edited),
            Err(Error::NativeWriteSizeChangeUnsupported)
        ));
    }

    #[test]
    fn rejects_size_changing_titles() {
        let baseline = notebook("Old");
        let edited = notebook("Longer");
        assert!(matches!(
            write_modified_section(b"Old", &baseline, &edited),
            Err(Error::NativeWriteSizeChangeUnsupported)
        ));
    }

    #[test]
    fn rejects_non_text_model_changes() {
        let baseline = notebook("Title");
        let mut edited = notebook("Title");
        let NotebookEntry::Section(section) = &mut edited.entries[0] else {
            unreachable!()
        };
        section.pages[0].level = 1;

        assert!(matches!(
            write_modified_section(b"Title", &baseline, &edited),
            Err(Error::NativeWriteUnsupported)
        ));
    }
}
