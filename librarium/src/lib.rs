//! Copy in/out file archives

use std::ffi::{CStr, CString};
use std::fmt::{self, Debug};
use std::io::{self, Cursor};
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::ParseIntError;

use deku::prelude::*;
use deku::writer::Writer;
use deku::DekuError;
use num::Integer;
use thiserror::Error;

const MAGIC: [u8; 6] = [b'0', b'7', b'0', b'7', b'0', b'1'];
const TRAILER: &str = "TRAILER!!!";

pub trait CpioHeader: for<'a> DekuReader<'a> + DekuWriter {
    fn into_header(&self) -> Header;
    fn from_header(header: Header, filesize: u64) -> Self;
    fn ino(&self) -> u32;
    fn mode(&self) -> u32;
    fn uid(&self) -> u32;
    fn gid(&self) -> u32;
    fn nlink(&self) -> u32;
    fn mtime(&self) -> u32;
    fn filesize(&self) -> u32;
    // fn devmajor(&self) -> u32;
    // fn devminor(&self) -> u32;
    // fn rdevmajor(&self) -> u32;
    // fn rdevminor(&self) -> u32;
    fn namesize(&self) -> u32;
    // fn check(&self) -> u32;
    fn name(&self) -> &str;
    fn data_pad(&self) -> usize;
}

// Much like DekuWriter, but lets us mutate ourself
trait MutWriter<Ctx = ()> {
    fn to_mutwriter<W: Write + Seek>(
        &mut self,
        deku_writer: &mut Writer<W>,
        ctx: Ctx,
    ) -> core::result::Result<(), DekuError>;
}

/// Errors generated from library
#[derive(Error, Debug)]
pub enum CpioError {
    #[error("std io error: {0}")]
    StdIo(#[from] io::Error),

    #[error("deku error: {0:?}")]
    Deku(#[from] deku::DekuError),
}

pub trait ReadSeek: Read + Seek {}
// pub trait BufReadSeek: BufRead + Seek + Send {}
impl<T: Read + Seek> ReadSeek for T {}

/// Private struct containing logic to read the data section from the archive
#[derive(Debug)]
pub(crate) struct ReaderWithOffset<R: ReadSeek> {
    io: R,
    /// Offset from start of file to squashfs
    offset: u64,
}

impl<R: ReadSeek> ReaderWithOffset<R> {
    pub fn new(mut io: R, offset: u64) -> std::io::Result<Self> {
        io.seek(SeekFrom::Start(offset))?;
        Ok(Self { io, offset })
    }
}

impl<R: ReadSeek> Read for ReaderWithOffset<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.io.read(buf)
    }
}

impl<R: ReadSeek> Seek for ReaderWithOffset<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let seek = match pos {
            SeekFrom::Start(start) => SeekFrom::Start(self.offset + start),
            seek => seek,
        };
        self.io.seek(seek).map(|x| x - self.offset)
    }
}

impl<T: ReadSeek> CpioReader for T {}
pub trait CpioReader: ReadSeek {
    fn extract_data<W, C: CpioHeader>(
        &mut self,
        object: &Object<C>,
        writer: &mut W,
    ) -> Result<(), CpioError>
    where
        W: Write + Seek,
    {
        // found the file, seek forward
        if let Data::Offset(offset) = object.data {
            self.seek(SeekFrom::Start(offset)).unwrap();
            let mut buf = vec![0; object.header.filesize() as usize];
            self.read_exact(&mut buf).unwrap();
            writer.write_all(&buf)?;
            Ok(())
        } else {
            panic!("no offset! TODO improve this");
        }
    }
}

pub enum Data {
    /// On read: Save current stream_position() as `Offset`, seek `header.filesize`
    /// This will be used to seek this this position if we want to extract *just* this file
    Offset(u64),
    /// On write: Write `Reader` to write buffer
    Reader(Box<dyn ReadSeek>),
}

impl DekuReader<'_, u32> for Data {
    fn from_reader_with_ctx<R: Read + Seek>(
        reader: &mut Reader<R>,
        filesize: u32,
    ) -> Result<Data, DekuError> {
        let reader = reader.as_mut();

        // Save the current offset, this is where the file exists for reading later
        let current_pos = reader.stream_position().unwrap();
        dbg!(current_pos);

        // Seek past that file
        let position = filesize as i64;
        dbg!(position);
        let _ = reader.seek(SeekFrom::Current(position));

        Ok(Self::Offset(current_pos))
    }
}

impl MutWriter<u32> for Data {
    fn to_mutwriter<W: Write + Seek>(
        &mut self,
        writer: &mut Writer<W>,
        _: u32,
    ) -> Result<(), DekuError> {
        if let Self::Reader(reader) = self {
            // read from reader
            let mut data = vec![];
            reader.read_to_end(&mut data).unwrap();

            // write to deku
            data.to_writer(writer, ())?;

            // add padding
            for _ in 0..pad_to_4(data.len()) {
                0_u8.to_writer(writer, ())?;
            }
        } else {
            panic!("ah");
        }

        Ok(())
    }
}

#[derive(DekuRead)]
pub struct Objects<C: CpioHeader> {
    #[deku(until = "Self::until")]
    pub inner: Vec<Object<C>>,
}

impl<C: CpioHeader> MutWriter for Objects<C> {
    fn to_mutwriter<W: Write + Seek>(
        &mut self,
        deku_writer: &mut Writer<W>,
        _: (),
    ) -> core::result::Result<(), DekuError> {
        for i in &mut self.inner {
            i.to_mutwriter(deku_writer, ())?;
        }
        Ok(())
    }
}

impl<C: CpioHeader> Objects<C> {
    fn until(last_object: &Object<C>) -> bool {
        last_object.header.name().as_bytes() == TRAILER.as_bytes()
    }
}

pub struct ArchiveReader<'b, C: CpioHeader> {
    pub reader: Box<dyn ReadSeek + 'b>,
    pub objects: Objects<C>,
}

impl<'b, C: CpioHeader> ArchiveReader<'b, C> {
    pub fn from_reader(reader: impl ReadSeek + 'b) -> Result<Self, CpioError> {
        Self::from_reader_with_offset(reader, 0)
    }

    pub fn from_reader_with_offset(
        reader: impl ReadSeek + 'b,
        offset: u64,
    ) -> Result<Self, CpioError> {
        let mut reader: Box<dyn ReadSeek> = if offset == 0 {
            Box::new(reader)
        } else {
            let reader = ReaderWithOffset::new(reader, offset)?;
            Box::new(reader)
        };
        let (_, objects) = Objects::from_reader((&mut reader, 0))?;
        Ok(Self { reader, objects })
    }

    pub fn extract_by_name<W>(
        &mut self,
        name: &str,
        writer: &mut W,
    ) -> Result<Option<Header>, CpioError>
    where
        W: Write + Seek,
    {
        for object in &self.objects.inner {
            if name == object.header.name() {
                self.reader.extract_data(object, writer)?;
                return Ok(Some(object.header.into_header()));
            }
        }

        Ok(None)
    }
}

pub trait WriteSeek: std::io::Write + Seek {}
impl<T: Write + Seek> WriteSeek for T {}

pub struct ArchiveWriter<'a, C: CpioHeader> {
    pub writer: Box<dyn WriteSeek + 'a>,
    pub objects: Objects<C>,
    pad_len: u32,
}

impl<'a, C: CpioHeader> ArchiveWriter<'a, C> {
    pub fn new(writer: Box<dyn WriteSeek + 'a>) -> Self {
        Self { writer, objects: Objects { inner: vec![] }, pad_len: 0x400 }
    }

    pub fn push_file(
        &mut self,
        mut reader: impl ReadSeek + 'a + 'static,
        header: Header,
    ) -> Result<(), CpioError> {
        // stream_len
        let filesize = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;

        let header = C::from_header(header, filesize);
        let object = Object::new(header, Data::Reader(Box::new(reader)));
        self.objects.inner.push(object);

        Ok(())
    }

    /// Before writing to Writer, a "TRAILER!!!" entry must be added
    // TODO: The new ASCII format is limited to 4 gigabyte file sizes.
    pub fn write(&mut self) -> Result<(), CpioError> {
        let header = Header { nlink: 1, name: "TRAILER!!!".to_string(), ..Default::default() };

        // empty data
        let data = Cursor::new(vec![]);
        self.push_file(data, header)?;

        let mut writer = Writer::new(&mut self.writer);
        self.objects.to_mutwriter(&mut writer, ()).unwrap();

        // pad bytes if required
        let bytes_used = (writer.bits_written / 8) as u64;
        if self.pad_len != 0 {
            // Pad out block_size to 4K
            let blocks_used: u32 = u32::try_from(bytes_used).unwrap() / self.pad_len;
            let total_pad_len = (blocks_used + 1) * self.pad_len;
            let pad_len = total_pad_len - u32::try_from(bytes_used).unwrap();

            // Write 1K at a time
            let mut total_written = 0;
            while ((writer.bits_written / 8) as u64) < (bytes_used + u64::from(pad_len)) {
                let arr = &[0x00; 1024];

                // check if last block to write
                let len = if (pad_len - total_written) < 1024 {
                    (pad_len - total_written) % 1024
                } else {
                    // else, full 1K
                    1024
                };

                writer.write_bytes(&arr[..len.try_into().unwrap()])?;
                total_written += len;
            }
        }

        Ok(())
    }
}

#[derive(Default)]
pub struct Header {
    pub ino: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub mtime: u32,
    pub devmajor: u32,
    pub devminor: u32,
    pub rdevmajor: u32,
    pub rdevminor: u32,
    pub name: String,
}

#[derive(DekuRead)]
pub struct Object<C: CpioHeader> {
    pub header: C,
    #[deku(ctx = "header.filesize()")]
    data: Data,
    #[deku(count = "header.data_pad()")]
    data_pad: Vec<u8>,
}

impl<C: CpioHeader> Object<C> {
    pub fn new(header: C, data: Data) -> Self {
        let data_pad = vec![0; header.data_pad()];
        Self { header, data, data_pad }
    }
}

impl<C: CpioHeader> MutWriter for Object<C> {
    fn to_mutwriter<W: Write + Seek>(
        &mut self,
        deku_writer: &mut Writer<W>,
        _: (),
    ) -> core::result::Result<(), DekuError> {
        DekuWriter::to_writer(&self.header, deku_writer, ())?;
        self.data.to_mutwriter(deku_writer, self.header.filesize())?;
        Ok(())
    }
}

pub const ODC_MAGIC: &[u8] = b"070707";

#[derive(DekuWrite, DekuRead, Debug)]
pub struct OdcHeader {
    #[deku(assert_eq = "ODC_MAGIC")]
    pub magic: [u8; 6],
    pub dev: Octal<u32, 6>,
    pub ino: Octal<u32, 6>,
    pub mode: Octal<u32, 6>,
    pub uid: Octal<u32, 6>,
    pub gid: Octal<u32, 6>,
    pub nlink: Octal<u32, 6>,
    pub rdev: Octal<u32, 6>,
    pub mtime: Octal<u64, 11>,
    pub namesize: Octal<u32, 6>,
    pub filesize: Octal<u64, 11>,
    #[deku(count = "namesize.value")]
    pub name: Vec<u8>,
}

impl CpioHeader for OdcHeader {
    fn from_header(header: Header, filesize: u64) -> Self {
        let name_bytes = header.name.into_bytes();
        let name_len = name_bytes.len();
        Self {
            magic: ODC_MAGIC.try_into().unwrap(),
            dev: Octal::new(header.devmajor),
            ino: Octal::new(header.ino),
            mode: Octal::new(header.mode),
            uid: Octal::new(header.uid),
            gid: Octal::new(header.gid),
            nlink: Octal::new(header.nlink),
            rdev: Octal::new(header.devminor),
            mtime: Octal::new(header.mtime.into()),
            namesize: Octal::new(name_len as u32),
            filesize: Octal::new(filesize),
            name: name_bytes.to_vec(),
        }
    }

    fn into_header(&self) -> Header {
        Header {
            ino: self.ino(),
            mode: self.mode(),
            uid: self.uid(),
            gid: self.gid(),
            nlink: self.nlink(),
            mtime: self.mtime(),
            devmajor: self.devmajor(),
            devminor: self.devminor(),
            rdevmajor: 0,
            rdevminor: 0,
            name: self.name().to_string(),
        }
    }

    fn ino(&self) -> u32 {
        self.ino.value
    }

    fn mode(&self) -> u32 {
        self.mode.value
    }

    fn uid(&self) -> u32 {
        self.uid.value
    }

    fn gid(&self) -> u32 {
        self.gid.value
    }

    fn nlink(&self) -> u32 {
        self.nlink.value
    }

    fn mtime(&self) -> u32 {
        self.mtime.value as u32
    }

    fn filesize(&self) -> u32 {
        self.filesize.value as u32
    }

    fn namesize(&self) -> u32 {
        self.namesize.value
    }

    // fn check(&self) -> u32 {
    //     todo!()
    // }

    fn name(&self) -> &str {
        CStr::from_bytes_with_nul(&self.name).unwrap().to_str().unwrap()
    }

    fn data_pad(&self) -> usize {
        0
    }
}

/// The new (SVR4) portable format, which supports file systems having more than 65536 i-nodes. (4294967295 bytes)
#[derive(DekuWrite, DekuRead, Debug)]
pub struct NewcHeader {
    #[deku(assert_eq = "MAGIC")]
    pub magic: [u8; 6],
    pub ino: Ascii,
    pub mode: Ascii,
    pub uid: Ascii,
    pub gid: Ascii,
    pub nlink: Ascii,
    pub mtime: Ascii,
    pub filesize: Ascii,
    pub devmajor: Ascii,
    pub devminor: Ascii,
    pub rdevmajor: Ascii,
    pub rdevminor: Ascii,
    pub namesize: Ascii,
    pub check: Ascii,
    // #[deku(assert = "name.as_bytes().len() == header.namesize.value as usize - 1")]
    // pub name: CString,
    #[deku(count = "namesize.value")]
    pub name: Vec<u8>,
    #[deku(count = "pad_to_4(6 + namesize.value as usize)")]
    name_pad: Vec<u8>,
}

impl CpioHeader for NewcHeader {
    fn from_header(header: Header, filesize: u64) -> Self {
        let name_bytes = header.name.into_bytes();
        let name_len = name_bytes.len();
        NewcHeader {
            magic: MAGIC,
            ino: Ascii::new(header.ino),
            mode: Ascii::new(header.mode),
            uid: Ascii::new(header.uid),
            gid: Ascii::new(header.gid),
            nlink: Ascii::new(header.nlink),
            mtime: Ascii::new(header.mtime),
            filesize: Ascii::new(u32::try_from(filesize).unwrap()),
            devmajor: Ascii::new(header.devmajor),
            devminor: Ascii::new(header.devminor),
            rdevmajor: Ascii::new(header.rdevmajor),
            rdevminor: Ascii::new(header.rdevminor),
            namesize: Ascii::new(name_len as u32 + 1),
            check: Ascii::new(0),
            name: name_bytes.to_vec(),
            name_pad: vec![0; pad_to_4(6 + name_len as usize)],
        }
    }

    fn into_header(&self) -> Header {
        Header {
            ino: self.ino(),
            mode: self.mode(),
            uid: self.uid(),
            gid: self.gid(),
            nlink: self.nlink(),
            mtime: self.mtime(),
            devmajor: self.devmajor.value,
            devminor: self.devminor.value,
            rdevmajor: 0,
            rdevminor: 0,
            name: self.name().to_string(),
        }
    }

    fn ino(&self) -> u32 {
        self.ino.value
    }

    fn mode(&self) -> u32 {
        self.mode.value
    }

    fn uid(&self) -> u32 {
        self.uid.value
    }

    fn gid(&self) -> u32 {
        self.gid.value
    }

    fn nlink(&self) -> u32 {
        self.nlink.value
    }

    fn mtime(&self) -> u32 {
        self.mtime.value
    }

    fn filesize(&self) -> u32 {
        self.filesize.value
    }

    fn devmajor(&self) -> u32 {
        self.devmajor.value
    }

    fn devminor(&self) -> u32 {
        self.devminor.value
    }

    // fn rdevmajor(&self) -> u32 {
    //     self.rdevmajor.value
    // }

    // fn rdevminor(&self) -> u32 {
    //     self.rdevminor.value
    // }

    fn namesize(&self) -> u32 {
        self.namesize.value
    }

    fn check(&self) -> u32 {
        self.check.value
    }

    fn name(&self) -> &str {
        CStr::from_bytes_with_nul(&self.name).unwrap().to_str().unwrap()
    }

    fn data_pad(&self) -> usize {
        pad_to_4(self.filesize() as usize)
    }
}

/// pad out to a multiple of 4 bytes
fn pad_to_4(len: usize) -> usize {
    match len % 4 {
        0 => 0,
        x => 4 - x,
    }
}

trait OctalConversion {
    fn to_octal_bytes(&self) -> Vec<u8>;
    fn from_octal_string(s: &str) -> Self;
}

impl<T> OctalConversion for T
where
    T: num_traits::PrimInt + num_traits::Zero + Debug,
{
    // Convert any integer type into an octal string
    fn to_octal_bytes(&self) -> Vec<u8> {
        let mut num = *self;
        let mut result = Vec::new();

        while num > T::zero() {
            let remainder = (num % T::from(8).unwrap()).to_u8().unwrap();
            result.push((b'0' + remainder) as u8);
            num = num / T::from(8).unwrap();
        }

        if result.is_empty() {
            result.push(b'0');
        }

        result.reverse();
        result
    }

    // Convert an octal string back to the integer type
    fn from_octal_string(s: &str) -> Self {
        let a = match T::from_str_radix(s, 8) {
            Ok(value) => value,
            Err(_) => T::zero(), // Or handle the error appropriately
        };
        dbg!(a);
        a
    }
}

#[derive(DekuWrite, DekuRead, Debug, Copy, Clone, Default)]
pub struct Octal<T: OctalConversion + fmt::Debug, const N: usize> {
    #[deku(reader = "Self::read(deku::reader)", writer = "self.write(deku::writer)")]
    pub value: T,
}

impl<T: OctalConversion + fmt::Debug, const N: usize> Octal<T, N> {
    pub fn new(value: T) -> Self {
        Self { value }
    }

    fn read<R: Read + Seek>(reader: &mut Reader<R>) -> Result<T, DekuError> {
        let value = <[u8; N]>::from_reader_with_ctx(reader, ())?;
        let s = std::str::from_utf8(&value).unwrap();
        dbg!(&s);
        let value = T::from_octal_string(s);
        dbg!(&value);
        Ok(value)
    }

    fn write<W: Write + Seek>(&self, writer: &mut Writer<W>) -> Result<(), DekuError> {
        let bytes = self.value.to_octal_bytes();
        writer.write_bytes(&bytes)?;

        Ok(())
    }
}

#[derive(DekuWrite, DekuRead, Debug, Copy, Clone, Default)]
pub struct Ascii {
    #[deku(reader = "Self::read(deku::reader)", writer = "self.write(deku::writer)")]
    pub value: u32,
}

impl Ascii {
    pub fn new(value: u32) -> Self {
        Self { value }
    }

    fn read<R: Read + Seek>(reader: &mut Reader<R>) -> Result<u32, DekuError> {
        let value = <[u8; 8]>::from_reader_with_ctx(reader, ())?;
        let s = core::str::from_utf8(&value).unwrap();
        let value = u32::from_str_radix(s, 16).unwrap();
        Ok(value)
    }

    // [30, 30, 38, 42, 32, 38, 37, 34]
    // "008B2874"
    fn write<W: Write + Seek>(&self, writer: &mut Writer<W>) -> Result<(), DekuError> {
        let bytes = self.value.to_be_bytes();
        for b in bytes {
            let left = (b & 0xf0) >> 4;
            let right = b & 0x0f;
            let left = if left > 9 { left + 0x37 } else { left + 0x30 };
            let right = if right > 9 { right + 0x37 } else { right + 0x30 };

            writer.write_bytes(&[left])?;
            writer.write_bytes(&[right])?;
        }

        Ok(())
    }
}
