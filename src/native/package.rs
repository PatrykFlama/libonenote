use super::write_modified_section;
use crate::model::{Notebook, NotebookEntry, Section};
use crate::{Error, LoadOptions, Result};
use cab::{Cabinet, CabinetBuilder, CompressionType};
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use time::{Date, Month, PrimitiveDateTime, Time};

struct PackageSnapshot {
    reserve_data: Vec<u8>,
    folders: Vec<PackageFolder>,
}

struct PackageFolder {
    compression: CompressionType,
    reserve_data: Vec<u8>,
    files: Vec<PackageFile>,
}

struct PackageFile {
    name: String,
    datetime: Option<PrimitiveDateTime>,
    is_read_only: bool,
    is_hidden: bool,
    is_system: bool,
    is_archive: bool,
    is_exec: bool,
    data: Vec<u8>,
}

pub(super) fn write_modified_package(
    original: &[u8],
    baseline: &Notebook,
    edited: &Notebook,
    options: LoadOptions,
) -> Result<Vec<u8>> {
    if !same_package_structure(baseline, edited) {
        return Err(Error::NativeWriteUnsupported);
    }

    let mut package = read_package(original)?;
    let baseline_sections = baseline.sections().collect::<Vec<_>>();
    let edited_sections = edited.sections().collect::<Vec<_>>();
    let mut matched_sections = HashSet::new();

    for file in package
        .folders
        .iter_mut()
        .flat_map(|folder| &mut folder.files)
        .filter(|file| extension(&file.name).eq_ignore_ascii_case("one"))
    {
        let parsed = crate::loader::load_section_buffer(&file.data, &file.name, options)?;
        let matches = baseline_sections
            .iter()
            .enumerate()
            .filter(|(_, section)| section_identity(section) == section_identity(&parsed))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [index] = matches.as_slice() else {
            if matches.is_empty() {
                continue;
            }
            return Err(Error::NativePackageSectionMapping(file.name.clone()));
        };
        if !matched_sections.insert(*index) {
            return Err(Error::NativePackageSectionMapping(file.name.clone()));
        }

        let section_baseline = section_notebook(&parsed);
        let section_edited = section_notebook(edited_sections[*index]);
        file.data = write_modified_section(&file.data, &section_baseline, &section_edited)?;
    }

    if matched_sections.len() != baseline_sections.len() {
        return Err(Error::NativePackageSectionMapping(
            "not every notebook section was found in the package".to_owned(),
        ));
    }

    write_package(package)
}

fn same_package_structure(baseline: &Notebook, edited: &Notebook) -> bool {
    baseline.name == edited.name
        && baseline.color == edited.color
        && same_entries(&baseline.entries, &edited.entries)
}

fn same_entries(baseline: &[NotebookEntry], edited: &[NotebookEntry]) -> bool {
    baseline.len() == edited.len()
        && baseline
            .iter()
            .zip(edited)
            .all(|(baseline, edited)| match (baseline, edited) {
                (NotebookEntry::Section(baseline), NotebookEntry::Section(edited)) => {
                    baseline.name == edited.name
                        && baseline.color == edited.color
                        && baseline.pages.len() == edited.pages.len()
                }
                (NotebookEntry::SectionGroup(baseline), NotebookEntry::SectionGroup(edited)) => {
                    baseline.name == edited.name && same_entries(&baseline.entries, &edited.entries)
                }
                _ => false,
            })
}

fn section_identity(section: &Section) -> (&str, Vec<&str>) {
    (
        &section.name,
        section.pages.iter().map(|page| page.id.as_str()).collect(),
    )
}

fn section_notebook(section: &Section) -> Notebook {
    Notebook {
        name: "Package section".to_owned(),
        color: None,
        entries: vec![NotebookEntry::Section(section.clone())],
    }
}

fn read_package(bytes: &[u8]) -> Result<PackageSnapshot> {
    let mut cabinet = Cabinet::new(Cursor::new(bytes))?;
    let reserve_data = cabinet.reserve_data().to_vec();
    let metadata = cabinet
        .folder_entries()
        .map(|folder| {
            let files = folder
                .file_entries()
                .map(|file| PackageFile {
                    name: file.name().to_owned(),
                    datetime: file.datetime(),
                    is_read_only: file.is_read_only(),
                    is_hidden: file.is_hidden(),
                    is_system: file.is_system(),
                    is_archive: file.is_archive(),
                    is_exec: file.is_exec(),
                    data: Vec::new(),
                })
                .collect();
            PackageFolder {
                compression: folder.compression_type(),
                reserve_data: folder.reserve_data().to_vec(),
                files,
            }
        })
        .collect::<Vec<_>>();

    let mut seen = HashSet::new();
    let mut folders = metadata;
    for file in folders.iter_mut().flat_map(|folder| &mut folder.files) {
        if !seen.insert(file.name.clone()) {
            return Err(Error::NativePackageSectionMapping(format!(
                "duplicate CAB entry {:?}",
                file.name
            )));
        }
        let mut reader = cabinet.read_file(&file.name)?;
        reader.read_to_end(&mut file.data)?;
    }
    Ok(PackageSnapshot {
        reserve_data,
        folders,
    })
}

fn write_package(package: PackageSnapshot) -> Result<Vec<u8>> {
    let mut builder = CabinetBuilder::new();
    builder.set_reserve_data(package.reserve_data);
    for folder in &package.folders {
        let output_folder = builder.add_folder(writable_compression(folder.compression));
        output_folder.set_reserve_data(folder.reserve_data.clone());
        for file in &folder.files {
            let output_file = output_folder.add_file(&file.name);
            output_file.set_datetime(file.datetime.unwrap_or_else(cab_epoch));
            output_file.set_is_read_only(file.is_read_only);
            output_file.set_is_hidden(file.is_hidden);
            output_file.set_is_system(file.is_system);
            output_file.set_is_archive(file.is_archive);
            output_file.set_is_exec(file.is_exec);
        }
    }

    let data = package
        .folders
        .into_iter()
        .flat_map(|folder| folder.files)
        .map(|file| file.data)
        .collect::<Vec<_>>();
    let mut writer = builder.build(Cursor::new(Vec::new()))?;
    let mut index = 0;
    while let Some(mut file) = writer.next_file()? {
        file.write_all(&data[index])?;
        index += 1;
    }
    Ok(writer.finish()?.into_inner())
}

fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

fn writable_compression(compression: CompressionType) -> CompressionType {
    match compression {
        CompressionType::None | CompressionType::MsZip => compression,
        CompressionType::Quantum(_, _) | CompressionType::Lzx(_) => CompressionType::MsZip,
    }
}

fn cab_epoch() -> PrimitiveDateTime {
    PrimitiveDateTime::new(
        Date::from_calendar_date(1980, Month::January, 1).expect("valid CAB epoch"),
        Time::MIDNIGHT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabinet_repack_preserves_entries_metadata_and_bytes() {
        let datetime = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::June, 22).unwrap(),
            Time::from_hms(18, 30, 0).unwrap(),
        );
        let package = PackageSnapshot {
            reserve_data: vec![1, 2, 3],
            folders: vec![PackageFolder {
                compression: CompressionType::MsZip,
                reserve_data: vec![4, 5],
                files: vec![
                    PackageFile {
                        name: "Open Notebook.onetoc2".to_owned(),
                        datetime: Some(datetime),
                        is_read_only: true,
                        is_hidden: false,
                        is_system: true,
                        is_archive: false,
                        is_exec: false,
                        data: b"toc".to_vec(),
                    },
                    PackageFile {
                        name: "Section.one".to_owned(),
                        datetime: Some(datetime),
                        is_read_only: false,
                        is_hidden: true,
                        is_system: false,
                        is_archive: true,
                        is_exec: false,
                        data: b"section".to_vec(),
                    },
                ],
            }],
        };

        let bytes = write_package(package).unwrap();
        let parsed = read_package(&bytes).unwrap();
        assert_eq!(parsed.reserve_data, vec![1, 2, 3]);
        assert_eq!(parsed.folders.len(), 1);
        assert_eq!(parsed.folders[0].reserve_data, vec![4, 5]);
        assert_eq!(parsed.folders[0].compression, CompressionType::MsZip);
        assert_eq!(parsed.folders[0].files[0].data, b"toc");
        assert_eq!(parsed.folders[0].files[1].data, b"section");
        assert!(parsed.folders[0].files[0].is_read_only);
        assert!(parsed.folders[0].files[0].is_system);
        assert!(parsed.folders[0].files[1].is_hidden);
        assert!(parsed.folders[0].files[1].is_archive);
        assert_eq!(parsed.folders[0].files[0].datetime, Some(datetime));
    }
}
