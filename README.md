# glob-rs

[![CI](https://github.com/VoiceLessQ/glob-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/VoiceLessQ/glob-rs/actions/workflows/ci.yml)

A Rust port of Python's [`glob`](https://docs.python.org/3/library/glob.html): shell-style
pathname expansion against the filesystem. It is built on the sibling
[`fnmatch-rs`](https://crates.io/crates/fnmatch-rs) crate, the same relationship CPython's
`glob` has with `fnmatch`.

| Pattern | Matches |
|---|---|
| `*` | any run of characters within one path segment (not a leading `.`) |
| `?` | any single character |
| `[seq]` / `[!seq]` | a character class (in / not in `seq`) |
| `**` | any number of directories, recursively (only with `recursive`) |

## Usage

```rust
use glob_rs::{glob, glob_with, escape, Options};

// Simple, relative to the current directory.
for path in glob("src/*.rs") {
    println!("{path}");
}

// Recursive, including hidden files, rooted elsewhere.
let opts = Options { recursive: true, include_hidden: true, root_dir: Some("data".into()) };
let all_txt = glob_with("**/*.txt", &opts);

// Treat a literal path containing wildcard characters as itself.
let pat = escape("weird[name].txt"); // -> "weird[[]name].txt"

// Get a regex for a glob (byte-identical to glob.translate; see the note below).
let _re: String = glob_rs::translate("**/*.py", true, false);
```

## Installation

```sh
cargo add glob-rs
```

Requires a Rust toolchain with 2024-edition support (Rust 1.85 or newer).

## Fidelity

The set of matched paths is faithful to CPython. Which paths match is decided by the same rules:
the `*?[` wildcard semantics, the leading-dot (hidden file) rule, and `**` recursing only when
`recursive` is set. Two peripheral details are functionally equivalent rather than byte-identical,
matching CPython's own documented contract:

- **Order is unspecified.** `glob.glob` says the same; sort the result if you need an order.
- **Separators are normalized to `/`.** Inputs may mix `/` and `\`; results use `/`.

On Windows, matching is case-insensitive, mirroring `os.path.normcase`. `iglob`/`iglob_with`
stream lazily like CPython's `iglob`: directories are read only as the iterator advances, so
`take`-ing a few matches from a huge tree never lists the whole thing. CPython's `dir_fd` argument
is not implemented (it needs `openat`-style syscalls Rust's std doesn't expose); use `root_dir`.

`translate` is exact too: its output is byte-identical to CPython's `glob.translate`. That regex
is Python/PCRE-flavored, so it uses lookahead (`(?!\.)`, `(?!)`) and `\Z`. Compile it with a
lookaround-capable engine such as `fancy-regex`, not the `regex` crate. (Filesystem globbing via
`glob` does not need it.)

## Verification

Behavior is checked by differential testing against CPython 3.13.13. The live `glob` module is the
oracle, and any disagreement is a bug in this crate.

- `glob` / `glob_with`: a fixture directory tree is matched with both this crate and `glob.glob`
  over a matrix of patterns × `recursive` × `include_hidden`, comparing the results as
  separator-normalized sets. All 264 invocations agree.
- `translate`: output is compared for byte-identical equality against `glob.translate` over the
  same matrix plus separator and character-class edge cases. All 356 outputs match.
- Plus 6 in-crate unit tests (`cargo test`), including a self-contained filesystem test.

```text
ALL MATCH - 264 glob invocations agree with Python glob.
ALL MATCH - 356 translate() outputs byte-identical to Python glob.translate.
```

The pre-existing `glob` crate implements Unix-shell semantics, not CPython's `glob` module; this
crate targets parity with the latter.

## License

Licensed under the [MIT License](LICENSE-MIT).
