use std::fs::{self, File, OpenOptions};
use std::io::SeekFrom;
use std::io::{BufReader, Seek};
use std::path::{Path, PathBuf};

use clap::Parser;
use librarium::{ArchiveReader, CpioReader, NewcHeader};

use clap::builder::styling::*;
pub fn styles() -> clap::builder::Styles {
    Styles::styled()
        .header(AnsiColor::Green.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
        .valid(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .invalid(AnsiColor::Yellow.on_default() | Effects::BOLD)
}

/// tool to extract and list cpio filesystems
#[derive(Parser)]
#[command(author,
          version,
          name = "uncpio",
          max_term_width = 98,
          styles = styles(),
)]
struct Args {
    /// CPIO path
    archive: Option<PathBuf>,

    /// Skip BYTES at the start of FILESYSTEM
    #[arg(short, long, default_value_t = 0, name = "BYTES")]
    offset: u64,

    /// Extract to [PATHNAME]
    #[arg(short, long, default_value = "out", name = "PATHNAME")]
    dest: PathBuf,
}

fn main() {
    let args = Args::parse();

    let mut file = BufReader::new(File::open(args.archive.as_ref().unwrap()).unwrap());
    file.seek(SeekFrom::Start(args.offset)).unwrap();

    // Extract all
    let mut archive: ArchiveReader<NewcHeader> =
        ArchiveReader::from_reader_with_offset(&mut file, args.offset).unwrap();
    for object in &archive.objects.inner {
        let filepath = Path::new(&args.dest).join(object.name.clone().into_string().unwrap());

        println!("extracting: {:?} -> {:02x?}", object.name, filepath);
        println!("{:?}", object.header);
        if object.header.filesize.value != 0 {
            let _ = fs::create_dir_all(filepath.parent().unwrap());
            let mut out =
                OpenOptions::new().write(true).create(true).truncate(true).open(filepath).unwrap();
            archive.reader.extract_data(object, &mut out).unwrap();
        }
    }
}
