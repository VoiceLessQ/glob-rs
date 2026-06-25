//! Differential harness binary for `difftest.py`. Two modes, both reading
//! `recursive<TAB>include_hidden<TAB>pattern` lines from stdin (flags are `0`/`1`):
//!
//! - `diff glob <root_dir>`: print the matched paths, joined by U+001F, one line per input.
//! - `diff translate`: print `glob_rs::translate(pattern, recursive, include_hidden)`, one line
//!   per input.
//!
//! Lines are separated by `\n`; exactly one output line is printed per input line. Trailing `\r`
//! is stripped so CRLF stdin (PowerShell / `subprocess(text=True)`) is tolerated.

use std::io::{self, Read, Write};

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let root = args.next().unwrap_or_default(); // glob mode only

    let mut input = String::new();
    io::stdin().read_to_string(&mut input).expect("read stdin");

    let mut out = String::new();
    for line in input.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 3 {
            out.push('\n');
            continue;
        }
        let recursive = parts[0] == "1";
        let include_hidden = parts[1] == "1";
        let pat = parts[2];

        if mode == "translate" {
            out.push_str(&glob_rs::translate(pat, recursive, include_hidden));
        } else {
            let opts = glob_rs::Options {
                recursive,
                include_hidden,
                root_dir: Some(root.clone()),
            };
            out.push_str(&glob_rs::glob_with(pat, &opts).join("\u{1f}"));
        }
        out.push('\n');
    }

    io::stdout().write_all(out.as_bytes()).expect("write stdout");
}
