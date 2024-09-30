use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::{ffi::CString, io::Cursor};

use librarium::NewcHeader;
use librarium::{ArchiveReader, ArchiveWriter, Header};
use test_assets::TestAssetDef;

// cpio -o -H newc > cpio-in.cpio
#[test]
fn test_simple_in_out_newc_files() {
    const TEST_PATH: &str = "test-assets/test_simple_in_out_newc/";
    let filepath = "cpio-in.cpio";
    let og_path = format!("{TEST_PATH}/{filepath}");
    let new_path = format!("{TEST_PATH}/bytes.squashfs");

    const FILE_NAME: &str = "cpio-in.cpio";
    let asset_defs = [TestAssetDef {
        filename: FILE_NAME.to_string(),
        hash: "39c7a5817e62fa451fb57638137bcfdd6add7fe706394d42c60c5c9314dc6cf2".to_string(),
        url: format!(
            "https://wcampbell.dev/cpio/testing/test_simple_in_out_newc_files/{FILE_NAME}"
        ),
    }];

    test_assets::download_test_files(&asset_defs, TEST_PATH, true).unwrap();

    let mut file = BufReader::new(File::open(&og_path).unwrap());
    let mut archive: ArchiveReader<NewcHeader> =
        ArchiveReader::from_reader_with_offset(&mut file, 0).unwrap();

    let a_assert = "a\n".as_bytes();
    let b_assert = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n".as_bytes();
    let c_assert = "cccccccccccccccccccccccccccccc\ncccc\nc\nc\nc\nc\nc\n".as_bytes();

    let mut a = Cursor::new(Vec::new());
    archive.extract_by_name(CString::new("cpio-in/a").unwrap(), &mut a).unwrap();
    assert_eq!(a.into_inner(), a_assert);

    let mut b = Cursor::new(Vec::new());
    archive.extract_by_name(CString::new("cpio-in/b").unwrap(), &mut b).unwrap();
    assert_eq!(b.into_inner(), b_assert);

    let mut c = Cursor::new(Vec::new());
    archive.extract_by_name(CString::new("cpio-in/c").unwrap(), &mut c).unwrap();
    assert_eq!(c.into_inner(), c_assert);

    let file = File::create(&new_path).unwrap();
    let mut writer: ArchiveWriter<NewcHeader> = ArchiveWriter::new(Box::new(file));

    // A
    let header_a = Header {
        ino: 9119860,
        mode: 33188,
        uid: 1000,
        gid: 1000,
        nlink: 1,
        mtime: 1703901104,
        devmajor: 0,
        devminor: 38,
        rdevmajor: 0,
        rdevminor: 0,
    };
    writer.push_file(Cursor::new(a_assert), CString::new("cpio-in/a").unwrap(), header_a).unwrap();

    // b
    let header_b = Header {
        ino: 9119861,
        mode: 33188,
        uid: 1000,
        gid: 1000,
        nlink: 1,
        mtime: 1703901110,
        devmajor: 0,
        devminor: 38,
        rdevmajor: 0,
        rdevminor: 0,
    };
    writer.push_file(Cursor::new(b_assert), CString::new("cpio-in/b").unwrap(), header_b).unwrap();

    // c
    let header_c = Header {
        ino: 9119863,
        mode: 33188,
        uid: 1000,
        gid: 1000,
        nlink: 1,
        mtime: 1703901119,
        devmajor: 0,
        devminor: 38,
        rdevmajor: 0,
        rdevminor: 0,
    };
    writer.push_file(Cursor::new(c_assert), CString::new("cpio-in/c").unwrap(), header_c).unwrap();

    writer.write().unwrap();

    let mut og_file = File::open(&og_path).unwrap();
    let mut new_file = File::open(&new_path).unwrap();

    let mut first = vec![];
    og_file.read_to_end(&mut first).unwrap();
    let mut second = vec![];
    new_file.read_to_end(&mut second).unwrap();

    assert_eq!(first, second);
}
