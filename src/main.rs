use clap::Parser;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::{
    fs::{remove_file, File, OpenOptions},
    io::{prelude::*, BufReader},
    net::TcpListener,
    process,
};
use tar::Builder;

/// woof in Rust, send any number of files/directories over a local network quickly
#[derive(Parser)]
struct Cli {
    /// Files or directories to send
    paths: Vec<std::path::PathBuf>,

    #[arg(short, long, default_value_t = String::from("127.0.0.1"))]
    ip: String,

    #[arg(short, long, default_value_t = 7878)]
    port: u16,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    const TEMP_FILE: &str = "woof-rs.tar.gz";
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
        // multiple files/dirs, prep the archive
        println!("Adding files/dirs to {TEMP_FILE}...");

        let tar_gz = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(TEMP_FILE)?;
        let enc = GzEncoder::new(tar_gz, Compression::fast());
        let mut tar = Builder::new(enc);

        for i in 0..cli.paths.len() {
            let current = cli.paths.get(i).unwrap();

            // add file to archive
            if current.is_file() {
                let mut f = File::open(current)?;
                tar.append_file(current, &mut f)?;
                keep_archive = true
            }
            // add dir to archive with dirname as last path component
            else if current.is_dir() {
                let dirname = current.file_name().unwrap().to_str().unwrap();
                let archive_dirname = format!("{dirname}-{i}");
                tar.append_dir_all(archive_dirname, current)?;
                keep_archive = true
            }
            // if neither, print error
            else {
                eprintln!("Error: {:?} is not a valid path", current);
            }
        }
        tar.finish()?;

        if !keep_archive {
            eprintln!("Error: Archive does not contain any files, exiting...");
            remove_file(TEMP_FILE)?;
            process::exit(-1);
        }

        println!("{TEMP_FILE} written successfully!");

        TEMP_FILE
    } else {
        // first path is invalid
        eprintln!("{:?} is not a valid path, exiting...", p_0);
        process::exit(-1);
    };

    // setup the response
    let f = File::open(f_name)?;
    let mut f_reader = BufReader::new(f);
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
    println!("Serving {} at http://{}", f_name, addr);

    // send the file on GET request
    let (mut stream, _) = listener.accept()?;
    let request = BufReader::new(&stream).lines().next().unwrap()?;
    if request == "GET / HTTP/1.1" {
        stream.write_all(&headers)?;
        // buffered write so we don't store the entire file in memory
        loop {
            let buf = f_reader.fill_buf()?; // default is 8KB
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
        remove_file(TEMP_FILE)?;
    }

    Ok(())
}
