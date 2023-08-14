use flate2::write::GzEncoder;
use flate2::Compression;
use std::{
    env,
    fs::{remove_file, File, OpenOptions},
    io::{prelude::*, BufReader},
    net::TcpListener,
    path::Path,
    process,
};
use tar::Builder;

fn main() -> std::io::Result<()> {
    const TEMP_FILE: &str = "woof-rs.tar.gz";

    let args: Vec<String> = env::args().collect();
    let f_path = Path::new(&args[1]);
    let f_name = if f_path.is_file() {
        f_path.to_str().unwrap()
    } else if f_path.is_dir() {
        // zip the directory
        println!("gzipping directory...");

        // overwrite existing temp file
        let tar_gz = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(TEMP_FILE)?;
        let enc = GzEncoder::new(tar_gz, Compression::fast());
        let mut tar = Builder::new(enc);
        tar.append_dir_all(f_path, f_path)?;
        tar.finish()?;
        println!("done!");

        TEMP_FILE
    } else {
        println!("Input is neither file nor directory, exiting...");
        process::exit(-1);
    };

    let f = File::open(f_name)?;
    let mut f_reader = BufReader::new(f);

    // setup the response
    let headers = [
        "HTTP/1.1 200 OK",
        "Content-Type: application/octet-stream",
        format!("Content-Disposition: attachment; filename=\"{}\"", f_name).as_str(),
        "\r\n",
    ]
    .join("\r\n")
    .into_bytes();

    // the web portion
    let addr = "127.0.0.1:7878";
    let listener = TcpListener::bind(addr)?;
    println!("Serving {} at {}", f_name, addr);

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
    remove_file(TEMP_FILE)?;

    Ok(())
}
