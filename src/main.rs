use clap::{Parser, ValueEnum};
use flate2::{write::GzEncoder, Compression};
use indicatif::ProgressBar;
use std::{
    fs::{remove_file, File},
    io::{prelude::*, BufReader, Result},
    net::TcpListener,
    path::PathBuf,
    process,
    str::FromStr,
};
use tar::Builder;
use walkdir::WalkDir;
use zip::{write::FileOptions, CompressionMethod::Zstd, ZipWriter};

/// Send any number of files/directories over a local network quickly
#[derive(Parser)]
struct Cli {
    /// Files or directories to send
    paths: Vec<PathBuf>,

    #[arg(
        short,
        long,
        value_enum,
        default_value_t = Encoding::Tgz,
        help="Archive encoding",
        long_help="Encoding to use when building archive for multiple files"
    )]
    enc: Encoding,

    /// Switch to receive mode
    #[arg(short, long)]
    recv: bool,

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
const BUF_SIZE: usize = 32 * 1024; // BufReader::new() default is 8KB = 8 * 1024
const OK_200: &str = "HTTP/1.1 200 OK";

fn main() -> Result<()> {
    let cli = Cli::parse();
    let send_html = include_bytes!("lib/send.html");
    let received_html = include_bytes!("lib/received.html");

    // bind port
    let addr = format!("{}:{}", cli.ip, cli.port);
    let listener = TcpListener::bind(&addr)?;

    // receive mode response
    let mut headers: Vec<String> = Vec::new();
    headers.push(OK_200.to_string());
    let mut f_name: Option<String> = None;
    let mut is_archive = false;

    // send mode response
    if !cli.recv {
        let (_f_name, _is_archive) = prepare_file(cli.enc, cli.paths);
        f_name = Some(_f_name.clone());
        is_archive = _is_archive;
        headers.push("Content-Type: application/octet-stream".to_string());
        headers.push(format!(
            "Content-Disposition: attachment; filename=\"{_f_name}\""
        ));

        println!("\nServing {} at http://{addr}", _f_name);
    } else {
        println!("\nWaiting to receive at http://{addr}");
    }

    // terminate headers
    headers.push("\r\n".to_string());

    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut request = BufReader::new(&mut stream);
        let mut start_line = String::new();
        request.read_line(&mut start_line)?;

        if start_line == "GET / HTTP/1.1\r\n" {
            // headers and body are already prepared depending on send/recv mode
            stream.write_all(&headers.join("\r\n").into_bytes())?;

            if cli.recv {
                stream.write_all(send_html)?;
            } else {
                let f = File::open(f_name.as_ref().unwrap())?;
                let mut f_buf = BufReader::with_capacity(BUF_SIZE, f);

                // TODO: verify if this is needed?
                // buffered write so we don't store the entire file in memory
                loop {
                    let buf = f_buf.fill_buf()?;
                    let length = buf.len();
                    if length == 0 {
                        break;
                    }
                    stream.write_all(buf)?;
                    f_buf.consume(length);
                }

                // stop after sending file
                break;
            }
        } else if start_line == "POST / HTTP/1.1\r\n" {
            // parse request headers, take_while stops on the first empty line
            // this consumes the request!
            let req_headers: Vec<String> = request
                .by_ref()
                .lines()
                .map(|result| result.unwrap())
                .take_while(|line| !line.is_empty())
                .collect();

            // get body metadata
            let mut content_length: usize = 0; // number of octets (8 bits, u8)
            let mut boundary = "";
            for line in &req_headers {
                if line.starts_with("Content-Length") {
                    content_length = line.split(":").last().unwrap().trim().parse().unwrap();
                }
                if line.starts_with("Content-Type") {
                    boundary = line
                        .split(";")
                        .last()
                        .unwrap()
                        .trim()
                        .split("=")
                        .last()
                        .unwrap();
                }
            }
            if content_length == 0 {
                break;
            }

            /*
            the response looks like this:
            ------WebKitFormBoundarymYhM14kHZ7UuBLfN
            Content-Disposition: form-data; name="upload-file"; filename="..."
            Content-Type: application/octet-stream

            <file contents>
            ------WebKitFormBoundarymYhM14kHZ7UuBLfN--

            boundary start: --boundarystring
            boundary end: --boundarystring--
            content-length includes the the boundaries
            */

            let mut filename = String::new();
            let mut bytes_consumed = 0;
            loop {
                // use this instead of map because we need to handle exact
                // amounts of bytes, and this is more granular
                let mut line = String::new();
                bytes_consumed += request.read_line(&mut line)?;
                if line.starts_with("Content-Disposition") {
                    filename = line
                        .split(";")
                        .last()
                        .unwrap()
                        .trim()
                        .split("=")
                        .last()
                        .unwrap()
                        .trim_matches('"')
                        .to_string();
                }
                // first empty line starts file content
                if line == "\r\n" {
                    break;
                }
            }

            /* this is different to Vec::with_capacity()
            https://stackoverflow.com/questions/68979882/readread-exact-does-not-fill-buffer

            - 2 bytes \r\n before boundary line
            - 2 bytes \r\n after boundary line
            - 4 bytes for -- wrapping boundary line on either side
            */

            let filesize = content_length - bytes_consumed - boundary.len() - 8;
            let mut buffer: Vec<u8> = vec![0; filesize];
            request.read_exact(&mut buffer)?;

            // write file
            let f_path = PathBuf::from_str(filename.as_str()).unwrap();
            if f_path.exists() {
                println!("Error: {:?} already exists", f_path);
            } else {
                let mut f = File::create(&f_path)?;
                f.write_all(&buffer)?;
                f.flush()?;

                println!("\n{:?} received", f_path);
            }

            // html templating at its finest :)
            let mut vec_received_html = received_html.to_vec();

            replace(&mut vec_received_html, b"{filename}", filename.as_bytes());
            replace(
                &mut vec_received_html,
                b"{bytes}",
                filesize.to_string().as_bytes(),
            );

            stream.write_all(OK_200.as_bytes())?;
            stream.write_all(b"\r\n\r\n")?;
            stream.write_all(&vec_received_html)?;

            break;
        }
    }

    // cleanup
    if is_archive {
        remove_file(f_name.unwrap())?;
    }

    Ok(())
}

// ---

// https://stackoverflow.com/questions/54150353/how-to-find-and-replace-every-matching-slice-of-bytes-with-another-slice
fn replace(source: &mut Vec<u8>, from: &[u8], to: &[u8]) {
    let from_len = from.len();
    let to_len = to.len();

    let mut i = 0;
    while i + from_len <= source.len() {
        if source[i..].starts_with(from) {
            source.splice(i..i + from_len, to.iter().cloned());
            i += to_len;
        } else {
            i += 1;
        }
    }
}

fn prepare_file(encoding: Encoding, paths: Vec<PathBuf>) -> (String, bool) {
    let temp_file = match encoding {
        Encoding::Tgz => {
            format!("{TEMP_FILE}.tar.gz")
        }
        Encoding::Zip => {
            format!("{TEMP_FILE}.zip")
        }
    };

    let mut is_archive = false;

    // if first path doesn't exist, exit
    let p_0 = match paths.get(0) {
        None => {
            println!("Error: No paths found, exiting...");
            process::exit(-1);
        }
        Some(p) => p,
    };

    let f_name = if paths.len() == 1 && p_0.is_file() {
        // 1 file only
        String::from(p_0.to_str().unwrap())
    } else if paths.len() > 1 || p_0.is_dir() {
        // multiple files/dirs
        println!("Adding files/dirs to {temp_file}...");
        is_archive = archive(&temp_file, encoding, paths).unwrap();
        println!("{temp_file} written successfully!");

        temp_file
    } else {
        // first path is invalid
        println!("{:?} is not a valid path, exiting...", p_0);
        process::exit(-1);
    };

    (f_name, is_archive)
}

// ---

fn archive(temp_file: &String, enc: Encoding, paths: Vec<PathBuf>) -> Result<bool> {
    let mut has_files = false;
    let f = File::create(temp_file)?;
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
        println!("Error: Archive does not contain any files, exiting...");
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
