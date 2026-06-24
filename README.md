# libonenote

`libonenote` is an unofficial, high-level Rust API for Microsoft OneNote files.
It is not affiliated with or endorsed by Microsoft.

The repository also includes an `onenote` CLI for inspecting, serializing, and
extracting notebook content.

Supported inputs:

- `.one` section files
- `.onepkg` notebook exports
- `.onetoc2` notebook indexes with adjacent section files

## Design

The crate returns an owned application-facing model. No
`onenote_parser` types appear in the public API, so the parsing backend can be
extended or replaced without forcing every application to change.

```rust
use libonenote::{BinaryDataPolicy, Loader};

let document = Loader::new()
    .binary_data(BinaryDataPolicy::MetadataOnly)
    .open("Notebook.onepkg")?;

for section in document.notebook().sections() {
    println!("{}: {} pages", section.name, section.pages.len());
}

# Ok::<(), libonenote::Error>(())
```

Binary objects and ink paths are metadata-only by default. Applications can
choose:

```rust
use libonenote::BinaryDataPolicy;

let metadata_only = BinaryDataPolicy::MetadataOnly;
let small_objects = BinaryDataPolicy::UpTo(16 * 1024 * 1024);
let everything = BinaryDataPolicy::All;
```

Complete ink paths are independently opt-in through
`Loader::ink_data(InkDataPolicy::All)`.

## CLI

```sh
cargo run -p onenote-cli -- info Notebook.onepkg
cargo run -p onenote-cli -- json Notebook.onepkg
cargo run -p onenote-cli -- extract Notebook.onepkg ./attachments
cargo run -p onenote-cli -- copy Section.one Section-copy.one
cargo run -p onenote-cli -- graph-export Notebook.onepkg ./graph-pages
cargo run -p onenote-cli -- rename-page Section.one Section-edited.one \
  "Old title" "New title"
cargo run -p onenote-cli -- replace-text Section.one Section-edited.one \
  "Old paragraph" "New paragraph"
cargo run -p onenote-cli -- replace-text Notebook.onepkg Notebook-edited.onepkg \
  "Old paragraph" "New paragraph"
```

With Nix:

```sh
nix run . -- info Notebook.onepkg
```

## Editing and saving

The high-level model is editable:

```rust
# use libonenote::Document;
# let mut document = Document::new("Notebook");
document.edit(|notebook| {
    notebook.name = "New name".to_owned();
});
```

Native OneNote writing is being implemented as verified, loss-preserving
updates over the original revision store:

- unchanged `.one` and `.onepkg` inputs can be copied byte-for-byte when
  `preserve_original(true)` was requested;
- `.one` page titles and complete paragraphs can be changed when the replacement
  fits the existing property allocation;
- `.onepkg` packages can be rebuilt with modified `.one` section payloads while
  preserving their TOC and unrelated CAB entries;
- UTF-16-only properties may be shortened; single-byte properties currently
  remain same-length;
- every generated `.one` is reparsed and rejected unless its page model exactly
  matches the requested result;
- structural edits, growing text, and resized single-byte text still return a
  hard error;
- `.onetoc2` notebooks are multi-file inputs and cannot be saved as one native
  file.

The writer changes only complete, length-prefixed text payloads in the preserved
file; all other bytes remain unchanged. Growing text, resizing single-byte
properties, ink, and object updates require allocating new revision-store
objects and are the next native-writer stage.

The CAB library can read OneNote's common LZX-compressed exports but cannot
encode LZX. Rebuilt LZX packages therefore use MSZIP compression; their
uncompressed entries and OneNote structure are preserved and verified.

### Microsoft Graph writer

`libonenote` can serialize the owned model into OneNote page XHTML and binary
multipart resources for Microsoft Graph:

```rust
use libonenote::{BinaryDataPolicy, GraphWriteOptions, Loader};

let document = Loader::new()
    .binary_data(BinaryDataPolicy::All)
    .open("Notebook.onepkg")?;
let export = document.to_graph_export(GraphWriteOptions::strict())?;

for page in export.pages {
    println!("{}: {} resources", page.title, page.resources.len());
}

# Ok::<(), libonenote::Error>(())
```

The writer is intentionally loss-aware:

- strict mode rejects editable ink and unknown native objects because the
  Microsoft Graph page API cannot recreate them faithfully;
- placeholder mode emits visible placeholders and structured warnings;
- images and attachments require their bytes to be loaded.

Authentication and HTTP synchronization belong in the consuming application.
`libonenote` produces request payloads and does not store tokens.

## Current backend

Parsing currently uses the MPL-2.0 `onenote_parser` crate internally. That
dependency is an implementation detail rather than part of this crate's public
model.

See [the architecture notes](docs/architecture.md) for the native writer plan.
