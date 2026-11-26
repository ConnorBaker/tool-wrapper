use object::LittleEndian;
use object::archive::{Header, MAGIC, TERMINATOR};
use object::elf::{FileHeader64, SectionHeader64};
use object::pod::from_bytes;
use object::read::elf::{FileHeader, SectionHeader};
use object::read::{ReadCache, ReadCacheOps, ReadRef};
use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::mem::offset_of;
use std::os::unix::fs::FileExt;

/// Bytes in a fixed-size memory buffer or an open, seekable file.
pub enum Artifact<'a> {
    Bytes(&'a mut [u8]),
    /// Use the length of the opened file as `size`. For positioned edits,
    /// open the file for writing with append mode disabled.
    File {
        file: &'a File,
        size: u64,
    },
}

impl Artifact<'_> {
    pub fn size(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File { size, .. } => *size,
        }
    }

    fn check_range(&self, offset: u64, count: u64) -> io::Result<()> {
        let size = self.size();
        if offset > size || count > size - offset {
            return Err(io::Error::other("byte range exceeds the artifact"));
        }
        Ok(())
    }

    pub fn read(&self, offset: u64, count: u64) -> io::Result<Vec<u8>> {
        self.read_bytes(offset, count).map(Cow::into_owned)
    }

    fn read_bytes(&self, offset: u64, count: u64) -> io::Result<Cow<'_, [u8]>> {
        self.check_range(offset, count)?;
        match self {
            Self::Bytes(bytes) => Ok(Cow::Borrowed(
                &bytes[offset as usize..(offset + count) as usize],
            )),
            Self::File { file, .. } => {
                let count = usize::try_from(count)
                    .map_err(|_| io::Error::other("byte range exceeds the address space"))?;
                let mut bytes = vec![0; count];
                file.read_exact_at(&mut bytes, offset)?;
                Ok(Cow::Owned(bytes))
            }
        }
    }

    pub fn write(&mut self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.check_range(offset, bytes.len() as u64)?;
        match self {
            Self::Bytes(output) => {
                let offset = offset as usize;
                output[offset..offset + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
            Self::File { file, .. } => file.write_all_at(bytes, offset),
        }
    }

    /// Rewrite the complete artifact. A callback error or `Ok(None)` leaves
    /// the bytes unchanged. Files can change length, while borrowed buffers
    /// must retain their original length.
    pub fn rewrite(
        &mut self,
        edit: impl FnOnce(&[u8]) -> io::Result<Option<Vec<u8>>>,
    ) -> io::Result<()> {
        let Some(bytes) = edit(&self.read_bytes(0, self.size())?)? else {
            return Ok(());
        };
        match self {
            Self::Bytes(output) => {
                if bytes.len() != output.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cannot resize a borrowed artifact",
                    ));
                }
                output.copy_from_slice(&bytes);
                Ok(())
            }
            Self::File { file, size } => {
                let new_size = bytes.len() as u64;
                if *size != new_size {
                    file.set_len(new_size)?;
                    *size = new_size;
                }
                file.write_all_at(&bytes, 0)
            }
        }
    }
}

pub fn is_elf(output: &Artifact<'_>) -> io::Result<bool> {
    let magic = object::elf::ELFMAG;
    let size = magic.len() as u64;
    Ok(output.size() >= size && *output.read_bytes(0, size)? == magic)
}

/// Update every file-backed section with this name, preserving its offset and size.
pub fn edit_elf_sections(
    output: &mut Artifact<'_>,
    name: &[u8],
    mut update: impl FnMut(&mut [u8]) -> bool,
) -> io::Result<bool> {
    let ranges = match output {
        Artifact::Bytes(bytes) => section_ranges(&**bytes, name),
        Artifact::File { file, size } => {
            let cache = ReadCache::new(PositionedFile {
                file,
                size: *size,
                offset: 0,
            });
            section_ranges(&cache, name)
        }
    }?;
    let mut changed = false;
    for (offset, size) in ranges {
        let mut contents = output.read(offset, size)?;
        if update(&mut contents) {
            output.write(offset, &contents)?;
            changed = true;
        }
    }
    Ok(changed)
}

fn section_ranges<'a>(data: impl ReadRef<'a>, name: &[u8]) -> io::Result<Vec<(u64, u64)>> {
    let malformed = || io::Error::other("the output is a malformed ELF file");
    let endian = LittleEndian;
    let header = data
        .read_at::<FileHeader64<LittleEndian>>(0)
        .map_err(|_| malformed())?;
    if header.e_ident.magic != object::elf::ELFMAG
        || !header.is_class_64()
        || !header.is_little_endian()
        || usize::from(header.e_shentsize(endian)) != size_of::<SectionHeader64<LittleEndian>>()
    {
        return Err(io::Error::other(
            "the output isn't a 64-bit little-endian ELF file",
        ));
    }
    if !header.is_supported() {
        return Err(malformed());
    }
    let sections = header.sections(endian, data).map_err(|_| malformed())?;
    if sections.is_empty() {
        return Err(malformed());
    }
    let len = data.len().map_err(|_| malformed())?;
    let mut ranges = Vec::new();
    for section in sections.iter() {
        if sections
            .section_name(endian, section)
            .map_err(|_| malformed())?
            == name
            && let Some((offset, size)) = section.file_range(endian)
        {
            if offset > len || size > len - offset {
                return Err(malformed());
            }
            ranges.push((offset, size));
        }
    }
    Ok(ranges)
}

// Share neither the caller's seek offset nor a whole-file mapping.
struct PositionedFile<'a> {
    file: &'a File,
    size: u64,
    offset: u64,
}

impl ReadCacheOps for PositionedFile<'_> {
    fn len(&mut self) -> Result<u64, ()> {
        Ok(self.size)
    }

    fn seek(&mut self, offset: u64) -> Result<u64, ()> {
        self.offset = offset;
        Ok(offset)
    }

    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()> {
        let size = self.file.read_at(bytes, self.offset).map_err(|_| ())?;
        self.offset += size as u64;
        Ok(size)
    }

    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), ()> {
        self.file
            .read_exact_at(bytes, self.offset)
            .map_err(|_| ())?;
        self.offset += bytes.len() as u64;
        Ok(())
    }
}

/// Zero the metadata in a regular archive after validating its member layout.
/// A write error can still leave an in-place file edit partially applied.
pub fn normalize_archive_metadata(output: &mut Artifact<'_>) -> io::Result<()> {
    const HEADER_SIZE: u64 = size_of::<Header>() as u64;
    const METADATA_OFFSET: u64 = offset_of!(Header, date) as u64;
    // The date, UID, and GID fields are adjacent.
    const METADATA: &[u8; 24] = b"0           0     0     ";
    let mut offset = MAGIC.len() as u64;
    if output.size() < offset || *output.read_bytes(0, offset)? != MAGIC {
        return Ok(());
    }

    let malformed = || io::Error::other("the output is a malformed ar archive");
    let mut edits = Vec::new();
    while offset < output.size() {
        let header = output
            .read_bytes(offset, HEADER_SIZE)
            .map_err(|_| malformed())?;
        let (fields, _) = from_bytes::<Header>(&header).map_err(|_| malformed())?;
        if fields.terminator != TERMINATOR {
            return Err(malformed());
        }
        let size = std::str::from_utf8(&fields.size)
            .ok()
            .map(|field| field.trim_end_matches(' '))
            .filter(|field| field.as_bytes().iter().all(u8::is_ascii_digit))
            .and_then(|size| size.parse::<u64>().ok())
            .ok_or_else(malformed)?;
        let padded_size = size.next_multiple_of(2);
        let payload = offset + HEADER_SIZE;
        if padded_size > output.size() - payload {
            return Err(malformed());
        }
        let old_metadata = &header[METADATA_OFFSET as usize..][..METADATA.len()];
        // The GNU long-name table can have blank date and ownership fields.
        let blank_names_metadata =
            fields.name.starts_with(b"//") && old_metadata.iter().all(|&b| b == b' ');
        if old_metadata != METADATA && !blank_names_metadata {
            edits.try_reserve(1).map_err(io::Error::other)?;
            edits.push(offset + METADATA_OFFSET);
        }
        offset = payload + padded_size;
    }
    for offset in edits {
        output.write(offset, METADATA)?;
    }
    Ok(())
}
