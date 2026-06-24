# libonenote architecture

## Goals

`libonenote` is the application API. Consumers should work with notebooks,
sections, pages, and content without understanding OneStore revision stores or
FSSHTTP packaging.

The current layers are:

```text
application / editor / CLI
            |
       libonenote model
        /           \
Graph XHTML writer  parser adapter (private)
                         |
                 OneStore / MS-ONE parser
```

The viewer still uses the parser and HTML renderer directly. A later change can
move it onto `libonenote` once the model exposes everything needed for faithful
rendering.

## Public model

The model is owned and editable. It includes:

- notebook section/group hierarchy;
- pages and subpage levels;
- positioned outlines;
- rich-text paragraphs and style runs;
- lists and tables;
- images and attachments;
- ink strokes;
- diagnostics.

Binary payload loading is explicit to prevent opening a large notebook from
unexpectedly consuming several gigabytes of memory.

## Native write strategy

The writer must not be implemented as a lossy model-to-file conversion.
OneNote files contain revision-store objects and properties that the public
model may not understand yet.

The implementation sequence is:

1. Preserve original bytes and prove byte-identical unchanged copies. Complete.
2. Implement verified fixed-allocation title and paragraph updates without
   rebuilding the revision store. Complete for `.one` sections.
3. Introduce a private lossless native document representation retaining
   unknown objects and properties.
4. Add mutation operations that record which native objects are affected.
5. Encode a minimal text-only section generated from scratch.
6. Validate generated files in Microsoft OneNote and against independent
   parsers.
7. Apply size-changing edits to preserved native documents by adding revisions
   while copying untouched records.
8. Add `.onepkg` CAB repacking around verified section updates. Complete;
   LZX inputs are emitted as MSZIP.
9. Add standalone `.onetoc2` multi-file notebook updates.

Unsupported mutations continue to return a hard error; the writer never falls
back to a lossy model-to-file conversion.

## Cloud write strategy

Cross-device OneNote synchronization should use Microsoft Graph rather than
placing reconstructed `.one` files into OneDrive storage.

`libonenote` owns the format conversion layer:

1. Convert pages to Graph-supported XHTML with absolute-positioned blocks.
2. Expose images and attachments as named multipart resources.
3. Report every unsupported or lossy conversion.
4. Keep authentication, tokens, retries, conflict handling, and HTTP transport
   outside the library.

Graph can create and patch page HTML, but it cannot create native editable ink
strokes. Ink therefore remains a native-writer objective. Applications may
choose strict rejection, omission, or visible placeholders until rasterized
ink export is added.

## API stability

Parser backend types must not be re-exported. New native-format functionality
belongs behind private adapters or a future low-level `onenote-format` crate.
The high-level `libonenote` model should remain suitable for GUI editors,
indexers, converters, and command-line tools.
