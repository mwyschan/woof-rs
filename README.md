# woof-rs

[woof](http://www.home.unix-ag.org/simon/woof.html)-inspired project, written in Rust.

Just a project for learning, not feature complete.

```console
➜ woof-rs --help
Send any number of files/directories over a local network quickly

Usage: woof-rs [OPTIONS] [PATHS]...

Arguments:
  [PATHS]...
          Files or directories to send

Options:
  -e, --enc <ENC>
          Encoding to use when building archive for multiple files

          [default: tgz]

          Possible values:
          - tgz: .tar.gz
          - zip: .zip

  -r, --recv
          Switch to receive mode

  -i, --ip <IP>
          [default: 127.0.0.1]

  -p, --port <PORT>
          [default: 7878]

  -h, --help
          Print help (see a summary with '-h')
```
