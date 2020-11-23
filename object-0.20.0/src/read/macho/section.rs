use core::fmt::Debug;
use core::{fmt, result, slice, str};

use crate::endian::{self, Endianness};
use crate::macho;
use crate::pod::{Bytes, Pod};
use crate::read::{
    self, CompressedData, ObjectSection, ReadError, Result, SectionFlags, SectionIndex, SectionKind,
};

use super::{MachHeader, MachOFile, MachORelocationIterator};

/// An iterator over the sections of a `MachOFile32`.
pub type MachOSectionIterator32<'data, 'file, Endian = Endianness> =
    MachOSectionIterator<'data, 'file, macho::MachHeader32<Endian>>;
/// An iterator over the sections of a `MachOFile64`.
pub type MachOSectionIterator64<'data, 'file, Endian = Endianness> =
    MachOSectionIterator<'data, 'file, macho::MachHeader64<Endian>>;

/// An iterator over the sections of a `MachOFile`.
pub struct MachOSectionIterator<'data, 'file, Mach>
where
    'data: 'file,
    Mach: MachHeader,
{
    pub(super) file: &'file MachOFile<'data, Mach>,
    pub(super) iter: slice::Iter<'file, MachOSectionInternal<'data, Mach>>,
}

impl<'data, 'file, Mach: MachHeader> fmt::Debug for MachOSectionIterator<'data, 'file, Mach> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // It's painful to do much better than this
        f.debug_struct("MachOSectionIterator").finish()
    }
}

impl<'data, 'file, Mach: MachHeader> Iterator for MachOSectionIterator<'data, 'file, Mach> {
    type Item = MachOSection<'data, 'file, Mach>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|&internal| MachOSection {
            file: self.file,
            internal,
        })
    }
}

/// A section of a `MachOFile32`.
pub type MachOSection32<'data, 'file, Endian = Endianness> =
    MachOSection<'data, 'file, macho::MachHeader32<Endian>>;
/// A section of a `MachOFile64`.
pub type MachOSection64<'data, 'file, Endian = Endianness> =
    MachOSection<'data, 'file, macho::MachHeader64<Endian>>;

/// A section of a `MachOFile`.
#[derive(Debug)]
pub struct MachOSection<'data, 'file, Mach>
where
    'data: 'file,
    Mach: MachHeader,
{
    pub(super) file: &'file MachOFile<'data, Mach>,
    pub(super) internal: MachOSectionInternal<'data, Mach>,
}

impl<'data, 'file, Mach: MachHeader> MachOSection<'data, 'file, Mach> {
    fn bytes(&self) -> Result<Bytes<'data>> {
        self.internal
            .section
            .data(self.file.endian, self.file.data)
            .read_error("Invalid Mach-O section size or offset")
    }
}

impl<'data, 'file, Mach: MachHeader> read::private::Sealed for MachOSection<'data, 'file, Mach> {}

impl<'data, 'file, Mach: MachHeader> ObjectSection<'data> for MachOSection<'data, 'file, Mach> {
    type RelocationIterator = MachORelocationIterator<'data, 'file, Mach>;

    #[inline]
    fn index(&self) -> SectionIndex {
        self.internal.index
    }

    #[inline]
    fn address(&self) -> u64 {
        self.internal.section.addr(self.file.endian).into()
    }

    #[inline]
    fn size(&self) -> u64 {
        self.internal.section.size(self.file.endian).into()
    }

    #[inline]
    fn align(&self) -> u64 {
        1 << self.internal.section.align(self.file.endian)
    }

    #[inline]
    fn file_range(&self) -> Option<(u64, u64)> {
        self.internal.section.file_range(self.file.endian)
    }

    #[inline]
    fn data(&self) -> Result<&'data [u8]> {
        Ok(self.bytes()?.0)
    }

    fn data_range(&self, address: u64, size: u64) -> Result<Option<&'data [u8]>> {
        Ok(read::data_range(
            self.bytes()?,
            self.address(),
            address,
            size,
        ))
    }

    #[inline]
    fn compressed_data(&self) -> Result<CompressedData<'data>> {
        self.data().map(CompressedData::none)
    }

    #[inline]
    fn name(&self) -> Result<&str> {
        str::from_utf8(self.internal.section.name())
            .ok()
            .read_error("Non UTF-8 Mach-O section name")
    }

    #[inline]
    fn segment_name(&self) -> Result<Option<&str>> {
        Ok(Some(
            str::from_utf8(self.internal.section.segment_name())
                .ok()
                .read_error("Non UTF-8 Mach-O segment name")?,
        ))
    }

    fn kind(&self) -> SectionKind {
        self.internal.kind
    }

    fn relocations(&self) -> MachORelocationIterator<'data, 'file, Mach> {
        MachORelocationIterator {
            file: self.file,
            relocations: self
                .internal
                .section
                .relocations(self.file.endian, self.file.data)
                .unwrap_or(&[])
                .iter(),
        }
    }

    fn flags(&self) -> SectionFlags {
        SectionFlags::MachO {
            flags: self.internal.section.flags(self.file.endian),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MachOSectionInternal<'data, Mach: MachHeader> {
    pub index: SectionIndex,
    pub kind: SectionKind,
    pub section: &'data Mach::Section,
}

impl<'data, Mach: MachHeader> MachOSectionInternal<'data, Mach> {
    pub(super) fn parse(index: SectionIndex, section: &'data Mach::Section) -> Self {
        // TODO: we don't validate flags, should we?
        let kind = match (section.segment_name(), section.name()) {
            (b"__TEXT", b"__text") => SectionKind::Text,
            (b"__TEXT", b"__const") => SectionKind::ReadOnlyData,
            (b"__TEXT", b"__cstring") => SectionKind::ReadOnlyString,
            (b"__TEXT", b"__literal4") => SectionKind::ReadOnlyData,
            (b"__TEXT", b"__literal8") => SectionKind::ReadOnlyData,
            (b"__TEXT", b"__literal16") => SectionKind::ReadOnlyData,
            (b"__TEXT", b"__eh_frame") => SectionKind::ReadOnlyData,
            (b"__TEXT", b"__gcc_except_tab") => SectionKind::ReadOnlyData,
            (b"__DATA", b"__data") => SectionKind::Data,
            (b"__DATA", b"__const") => SectionKind::ReadOnlyData,
            (b"__DATA", b"__bss") => SectionKind::UninitializedData,
            (b"__DATA", b"__common") => SectionKind::Common,
            (b"__DATA", b"__thread_data") => SectionKind::Tls,
            (b"__DATA", b"__thread_bss") => SectionKind::UninitializedTls,
            (b"__DATA", b"__thread_vars") => SectionKind::TlsVariables,
            (b"__DWARF", _) => SectionKind::Debug,
            _ => SectionKind::Unknown,
        };
        MachOSectionInternal {
            index,
            kind,
            section,
        }
    }
}

/// A trait for generic access to `Section32` and `Section64`.
#[allow(missing_docs)]
pub trait Section: Debug + Pod {
    type Word: Into<u64>;
    type Endian: endian::Endian;

    fn sectname(&self) -> &[u8; 16];
    fn segname(&self) -> &[u8; 16];
    fn addr(&self, endian: Self::Endian) -> Self::Word;
    fn size(&self, endian: Self::Endian) -> Self::Word;
    fn offset(&self, endian: Self::Endian) -> u32;
    fn align(&self, endian: Self::Endian) -> u32;
    fn reloff(&self, endian: Self::Endian) -> u32;
    fn nreloc(&self, endian: Self::Endian) -> u32;
    fn flags(&self, endian: Self::Endian) -> u32;

    /// Return the `sectname` bytes up until the null terminator.
    fn name(&self) -> &[u8] {
        let sectname = &self.sectname()[..];
        match sectname.iter().position(|&x| x == 0) {
            Some(end) => &sectname[..end],
            None => sectname,
        }
    }

    /// Return the `segname` bytes up until the null terminator.
    fn segment_name(&self) -> &[u8] {
        let segname = &self.segname()[..];
        match segname.iter().position(|&x| x == 0) {
            Some(end) => &segname[..end],
            None => segname,
        }
    }

    /// Return the offset and size of the section in the file.
    ///
    /// Returns `None` for sections that have no data in the file.
    fn file_range(&self, endian: Self::Endian) -> Option<(u64, u64)> {
        match self.flags(endian) & macho::SECTION_TYPE {
            macho::S_ZEROFILL | macho::S_GB_ZEROFILL | macho::S_THREAD_LOCAL_ZEROFILL => None,
            _ => Some((self.offset(endian).into(), self.size(endian).into())),
        }
    }

    /// Return the section data.
    ///
    /// Returns `Ok(&[])` if the section has no data.
    /// Returns `Err` for invalid values.
    fn data<'data>(
        &self,
        endian: Self::Endian,
        data: Bytes<'data>,
    ) -> result::Result<Bytes<'data>, ()> {
        if let Some((offset, size)) = self.file_range(endian) {
            data.read_bytes_at(offset as usize, size as usize)
        } else {
            Ok(Bytes(&[]))
        }
    }

    /// Return the relocation array.
    ///
    /// Returns `Err` for invalid values.
    fn relocations<'data>(
        &self,
        endian: Self::Endian,
        data: Bytes<'data>,
    ) -> Result<&'data [macho::Relocation<Self::Endian>]> {
        data.read_slice_at(self.reloff(endian) as usize, self.nreloc(endian) as usize)
            .read_error("Invalid Mach-O relocations offset or number")
    }
}

impl<Endian: endian::Endian> Section for macho::Section32<Endian> {
    type Word = u32;
    type Endian = Endian;

    fn sectname(&self) -> &[u8; 16] {
        &self.sectname
    }
    fn segname(&self) -> &[u8; 16] {
        &self.segname
    }
    fn addr(&self, endian: Self::Endian) -> Self::Word {
        self.addr.get(endian)
    }
    fn size(&self, endian: Self::Endian) -> Self::Word {
        self.size.get(endian)
    }
    fn offset(&self, endian: Self::Endian) -> u32 {
        self.offset.get(endian)
    }
    fn align(&self, endian: Self::Endian) -> u32 {
        self.align.get(endian)
    }
    fn reloff(&self, endian: Self::Endian) -> u32 {
        self.reloff.get(endian)
    }
    fn nreloc(&self, endian: Self::Endian) -> u32 {
        self.nreloc.get(endian)
    }
    fn flags(&self, endian: Self::Endian) -> u32 {
        self.flags.get(endian)
    }
}

impl<Endian: endian::Endian> Section for macho::Section64<Endian> {
    type Word = u64;
    type Endian = Endian;

    fn sectname(&self) -> &[u8; 16] {
        &self.sectname
    }
    fn segname(&self) -> &[u8; 16] {
        &self.segname
    }
    fn addr(&self, endian: Self::Endian) -> Self::Word {
        self.addr.get(endian)
    }
    fn size(&self, endian: Self::Endian) -> Self::Word {
        self.size.get(endian)
    }
    fn offset(&self, endian: Self::Endian) -> u32 {
        self.offset.get(endian)
    }
    fn align(&self, endian: Self::Endian) -> u32 {
        self.align.get(endian)
    }
    fn reloff(&self, endian: Self::Endian) -> u32 {
        self.reloff.get(endian)
    }
    fn nreloc(&self, endian: Self::Endian) -> u32 {
        self.nreloc.get(endian)
    }
    fn flags(&self, endian: Self::Endian) -> u32 {
        self.flags.get(endian)
    }
}
