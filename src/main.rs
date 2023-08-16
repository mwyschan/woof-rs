use clap::{Parser, ValueEnum};
use flate2::{write::GzEncoder, Compression};
use indicatif::ProgressBar;
use std::{
    fs::{remove_file, File, OpenOptions},
    io::{prelude::*, BufReader, Result},
    net::TcpListener,
    path::PathBuf,
    process,
    str::FromStr,
};
use tar::Builder;
use walkdir::WalkDir;
use zip::{write::FileOptions, CompressionMethod::Zstd, ZipWriter};

/// woof in Rust, send any number of files/directories over a local network quickly
#[derive(Parser)]
struct Cli {
    /// Files or directories to send
    paths: Vec<PathBuf>,

    #[arg(
        value_enum,
        short,
        long,
        default_value_t = Encoding::Tgz,
        help="Archive encoding",
        long_help="Encoding to use when building archive for multiple files"
    )]
    encoding: Encoding,

    #[arg(short, long, default_value_t = String::from("127.0.0.1"))]
    ip: String,

    #[arg(short, long, default_value_t = 7878)]
    port: u16,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Encoding {
    /// .tar.gz
    Tgz,
    /// .zip
    Zip,
}

const TEMP_FILE: &str = "woof-rs";
const BUF_SIZE: usize = 32 * 1024; // BufReader::new(f) default is 8KB = 8 * 1024

fn main() -> Result<()> {
    let cli = Cli::parse();
    let temp = match cli.encoding {
        Encoding::Tgz => {
            format!("{TEMP_FILE}.tar.gz")
        }
        Encoding::Zip => {
            format!("{TEMP_FILE}.zip")
        }
    };
    let temp_file = temp.as_str();

    let mut keep_archive = false;

    // if first path doesn't exist, exit
    let p_0 = match cli.paths.get(0) {
        None => {
            eprintln!("Error: No paths found, exiting...");
            process::exit(-1);
        }
        Some(p) => p,
    };

    let f_name = if cli.paths.len() == 1 && p_0.is_file() {
        // 1 file only
        p_0.to_str().unwrap()
    } else if cli.paths.len() > 1 || p_0.is_dir() {
        // multiple files/dirs
        println!("Adding files/dirs to {temp_file}...");
        keep_archive = archive(temp_file, cli.encoding, cli.paths)?;
        println!("{temp_file} written successfully!");

        temp_file
    } else {
        // first path is invalid
        eprintln!("{:?} is not a valid path, exiting...", p_0);
        process::exit(-1);
    };

    // setup the response
    let f = File::open(f_name)?;
    // let f_size = f.metadata()?.len();
    // let buf_size: u64 = BUF_SIZE.try_into().unwrap();
    // let chunks: u64 = (f_size + buf_size - 1) / buf_size; // ceiling div
    let mut f_reader = BufReader::with_capacity(BUF_SIZE, f);
    let headers = [
        "HTTP/1.1 200 OK",
        "Content-Type: application/octet-stream",
        format!("Content-Disposition: attachment; filename=\"{f_name}\"").as_str(),
        "\r\n",
    ]
    .join("\r\n")
    .into_bytes();

    // the web portion
    let addr = format!("{}:{}", cli.ip, cli.port);
    let listener = TcpListener::bind(&addr)?;
    println!("\nServing {} at http://{}", f_name, addr);

    // send the file on GET request
    let (mut stream, _) = listener.accept()?;
    let request = BufReader::new(&stream).lines().next().unwrap()?;
    if request == "GET / HTTP/1.1" {
        stream.write_all(&headers)?;
        // buffered write so we don't store the entire file in memory
        loop {
            let buf = f_reader.fill_buf()?;
            let length = buf.len();
            if length == 0 {
                break;
            }
            stream.write_all(buf)?;
            f_reader.consume(length);
        }
    }

    // cleanup
    if keep_archive {
        remove_file(temp_file)?;
    }

    Ok(())
}

fn archive(temp_file: &str, enc: Encoding, paths: Vec<PathBuf>) -> Result<bool> {
    let mut has_files = false;
    let f = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(temp_file)?;
    let bar = ProgressBar::new(paths.len().try_into().unwrap());

    match enc {
        Encoding::Tgz => {
            let enc = GzEncoder::new(f, Compression::fast());
            let mut tar = Builder::new(enc);

            for (i, path) in paths.iter().enumerate() {
                let p = path.file_name().unwrap().to_str().unwrap();

                // add file to archive
                if path.is_file() {
                    let mut f = File::open(path)?;
                    tar.append_file(p, &mut f)?;
                    has_files = true
                }
                // add dir to archive with dirname as last path component
                else if path.is_dir() {
                    let dirname = format!("{p}-{i}");
                    tar.append_dir_all(dirname, path).unwrap();
                    has_files = true
                }
                // if neither, print error
                else {
                    println!("Error: {:?} is not a valid path", path);
                }

                bar.inc(1);
            }
            tar.finish()?;
        }

        Encoding::Zip => {
            let mut zip = ZipWriter::new(f);
            let options = FileOptions::default()
                .compression_method(Zstd)
                .unix_permissions(0o755);

            // zip is confusing because you have to encode each file separately
            for (i, path) in paths.iter().enumerate() {
                let p = path.file_name().unwrap().to_str().unwrap();

                // add file to archive
                if path.is_file() {
                    let f = File::open(path)?;
                    zip_add_file(&mut zip, f, p, options)?;
                    has_files = true;
                }
                // add dir to archive with dirname as last path component
                else if path.is_dir() {
                    let dirname = PathBuf::from_str(format!("{p}-{i}").as_str()).unwrap();

                    let walkdir = WalkDir::new(path);
                    for entry in walkdir.into_iter() {
                        let e = entry?;
                        let e_path = e.path();
                        let e_child = e_path.strip_prefix(path.as_path()).unwrap();
                        let e_new = dirname.join(e_child);

                        // walkdir starts from the topmost directory, so e_child is empty
                        match e_child.file_name() {
                            Some(_) => {
                                if e_path.is_file() {
                                    let f = File::open(e_path)?;
                                    zip_add_file(&mut zip, f, e_new.to_str().unwrap(), options)?;
                                } else if e_path.is_dir() {
                                    zip.add_directory(e_new.to_str().unwrap(), options)?;
                                }
                            }
                            None => {
                                // topmost dir
                                zip.add_directory(dirname.to_str().unwrap(), options)?;
                            }
                        }
                    }
                    has_files = true
                }
                // if neither, print error
                else {
                    println!("Error: {:?} is not a valid path", path);
                }
            }
            zip.finish()?;
        }
    }

    if !has_files {
        eprintln!("Error: Archive does not contain any files, exiting...");
        remove_file(temp_file)?;
        process::exit(-1);
    }

    Ok(has_files)
}

fn zip_add_file(zip: &mut ZipWriter<File>, f: File, p: &str, options: FileOptions) -> Result<()> {
    let mut f_reader = BufReader::new(f);
    zip.start_file(p, options)?;
    loop {
        let buf = f_reader.fill_buf()?;
        let length = buf.len();
        if length == 0 {
            break;
        }
        zip.write_all(buf)?;
        f_reader.consume(length);
    }
    Ok(())
}
