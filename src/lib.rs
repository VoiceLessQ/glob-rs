//! A Rust port of Python's [`glob`](https://docs.python.org/3/library/glob.html):
//! shell-style pathname expansion against the real filesystem.
//!
//! Port target: CPython's standard-library `glob.py` (the classic `glob`/`iglob` path:
//! `_iglob` calling `_glob0`/`_glob1`/`_glob2`). Wildcard matching is delegated to the sibling
//! [`fnmatch-rs`] crate, the same port relationship CPython has between `glob` and `fnmatch`.
//!
//! # Fidelity
//!
//! The *result set* is faithful to CPython. Which paths match is decided by the same rules:
//! the `*?[` wildcard semantics, the leading-dot (hidden file) rule, and `**` recursing only
//! when `recursive` is set. Peripheral details are functionally equivalent rather than
//! byte-identical: the order of results is unspecified (CPython says so too), and path strings
//! are assembled with `/` separators regardless of the input's separators. On Windows, matching
//! is case-insensitive, mirroring `os.path.normcase`.
//!
//! CPython's `dir_fd` argument (search relative to an open directory file descriptor) is not
//! implemented: it needs `openat`/`fstatat`-style syscalls that Rust's standard library does not
//! expose, and pulling in `libc`/`rustix` would break the near-zero-dependency design. Use
//! `root_dir` instead, which covers the common "search relative to a directory" case.
//!
//! [`fnmatch-rs`]: https://crates.io/crates/fnmatch-rs

use regex::Regex;
use std::path::Path;

/// Options for [`glob_with`], mirroring the keyword arguments of Python's `glob.glob`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// When set, the pattern segment `**` matches any number of directories, recursively.
    /// Without it, `**` behaves like `*` (a single segment).
    pub recursive: bool,
    /// When set, wildcards (`*`, `?`, `**`) also match names beginning with a dot.
    pub include_hidden: bool,
    /// Root directory to search from. `None` means the current working directory. Results are
    /// returned relative to this root, exactly like CPython's `root_dir` argument.
    pub root_dir: Option<String>,
}

/// Return the paths matching `pathname`, with default options (non-recursive, hidden files
/// excluded, rooted at the current directory). Port of `glob.glob`.
///
/// The order of the returned list is unspecified; sort it if you need a particular order.
pub fn glob(pathname: &str) -> Vec<String> {
    glob_with(pathname, &Options::default())
}

/// Return the paths matching `pathname` under the given [`Options`]. Port of `glob.glob`.
pub fn glob_with(pathname: &str, opts: &Options) -> Vec<String> {
    iglob_with(pathname, opts).collect()
}

/// Iterator form of [`glob`]. Port of `glob.iglob`.
///
/// Like CPython's `iglob`, this streams lazily: directories are read only as the iterator is
/// advanced, so matching a huge tree (or `take`-ing a few results) never materializes the whole
/// list. (Within a single directory the entries are read eagerly, matching CPython's `_listdir`.)
pub fn iglob(pathname: &str) -> Box<dyn Iterator<Item = String>> {
    iglob_with(pathname, &Options::default())
}

/// Iterator form of [`glob_with`]. See [`iglob`] for the note on laziness.
pub fn iglob_with(pathname: &str, opts: &Options) -> Box<dyn Iterator<Item = String>> {
    let root = opts.root_dir.as_deref().unwrap_or("");
    let inner = iglob_inner(pathname, root, opts.recursive, false, opts.include_hidden);
    // `iglob` skips the leading empty string that `_glob2` yields for the root itself. Peeking one
    // element keeps the rest lazy.
    let skips_root = pathname.is_empty() || (opts.recursive && pathname.starts_with("**"));
    if skips_root {
        let mut it = inner.peekable();
        if it.peek().is_some_and(|first| first.is_empty()) {
            it.next();
        }
        Box::new(it)
    } else {
        inner
    }
}

/// Escape all special characters in `pathname` so it can be used as a literal path with
/// [`glob`]. Each of `*`, `?`, `[` is wrapped in a character class. Port of `glob.escape`.
pub fn escape(pathname: &str) -> String {
    let (drive, rest) = splitdrive(pathname);
    let mut out = String::with_capacity(rest.len());
    for c in rest.chars() {
        if matches!(c, '*' | '?' | '[') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    format!("{drive}{out}")
}

/// Whether `s` contains any shell-glob magic character (`*`, `?`, `[`). Port of `glob.has_magic`.
pub fn has_magic(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}

/// Translate a glob `pat` into a regular-expression string. Port of `glob.translate`.
///
/// The returned regex is **Python/PCRE-flavored**, byte-identical to CPython's output: it uses
/// lookahead (`(?!\.)`, `(?!)`) for the hidden-file and empty-class guards and `\Z` for
/// end-of-string. Compile it with an engine that supports lookaround (e.g. `fancy-regex`); the
/// `regex` crate has no lookaround and cannot compile it. For matching against the filesystem use
/// [`glob`], which does not need this. Separators follow the platform (Windows: `/` and `\`;
/// elsewhere `/`), exactly like `os.path.sep`/`os.path.altsep`.
///
/// With `recursive`, a `**` path segment matches any number of segments. With `include_hidden`,
/// wildcards also match segments beginning with a dot.
pub fn translate(pat: &str, recursive: bool, include_hidden: bool) -> String {
    let escaped_seps: String = SEPS.iter().map(|&c| re_escape_char(c)).collect();
    let any_sep = if SEPS.len() > 1 {
        format!("[{escaped_seps}]")
    } else {
        escaped_seps.clone()
    };
    let not_sep = format!("[^{escaped_seps}]");

    let (one_last_segment, one_segment, any_segments, any_last_segments) = if include_hidden {
        let one_last = format!("{not_sep}+");
        let one = format!("{one_last}{any_sep}");
        (one_last, one, format!("(?:.+{any_sep})?"), ".*".to_string())
    } else {
        let one_last = format!("[^{escaped_seps}.]{not_sep}*");
        let one = format!("{one_last}{any_sep}");
        let any = format!("(?:{one})*");
        let any_last = format!("{any}(?:{one_last})?");
        (one_last, one, any, any_last)
    };

    let star = format!("{not_sep}*");
    let parts: Vec<&str> = pat.split(|c| SEPS.contains(&c)).collect();
    let last = parts.len() - 1;

    let mut results = String::new();
    for (idx, part) in parts.iter().enumerate() {
        if *part == "*" {
            results.push_str(if idx < last { &one_segment } else { &one_last_segment });
        } else if recursive && *part == "**" {
            if idx < last {
                if parts[idx + 1] != "**" {
                    results.push_str(&any_segments);
                }
            } else {
                results.push_str(&any_last_segments);
            }
        } else {
            if !part.is_empty() {
                if !include_hidden && matches!(part.chars().next(), Some('*' | '?')) {
                    results.push_str("(?!\\.)");
                }
                results.push_str(&translate_part(part, &star, &not_sep));
            }
            if idx < last {
                results.push_str(&any_sep);
            }
        }
    }
    format!("(?s:{results})\\Z")
}

/// Platform path separators, matching `os.path.sep` / `os.path.altsep`.
#[cfg(windows)]
const SEPS: &[char] = &['\\', '/'];
#[cfg(not(windows))]
const SEPS: &[char] = &['/'];

/// Port of `fnmatch._translate` (the parameterized core): translate a single glob segment, using
/// `star` for each `*` run and `question_mark` for each `?`, returning the joined regex body.
/// This is the Python-regex-flavored sibling of [`fnmatch_rs::translate`] (which targets the
/// `regex` crate); it is kept local so [`translate`] can stay byte-identical to CPython.
fn translate_part(pat: &str, star: &str, question_mark: &str) -> String {
    let chars: Vec<char> = pat.chars().collect();
    let n = chars.len();
    let mut res = String::new();
    let mut last_was_star = false;
    let mut i = 0;
    while i < n {
        let c = chars[i];
        i += 1;
        if c == '*' {
            if !last_was_star {
                res.push_str(star);
            }
            last_was_star = true;
            continue;
        }
        last_was_star = false;
        match c {
            '?' => res.push_str(question_mark),
            '[' => i = translate_class(&chars, i, n, &mut res),
            _ => res.push_str(&re_escape_char(c)),
        }
    }
    res
}

/// Translate a `[...]` character class (Python-regex flavor). Mirrors the `[` branch of
/// `fnmatch._translate`: an empty class becomes `(?!)` (never matches) and a negated-empty class
/// becomes `.`.
fn translate_class(chars: &[char], i: usize, n: usize, body: &mut String) -> usize {
    let mut j = i;
    if j < n && chars[j] == '!' {
        j += 1;
    }
    if j < n && chars[j] == ']' {
        j += 1;
    }
    while j < n && chars[j] != ']' {
        j += 1;
    }
    if j >= n {
        body.push_str("\\["); // unterminated `[` is a literal bracket
        return i;
    }

    let stuff = build_class_stuff(chars, i, j);
    let next = j + 1;
    if stuff.is_empty() {
        body.push_str("(?!)");
    } else if stuff == "!" {
        body.push('.');
    } else {
        let mut s = stuff;
        let first = s.chars().next().unwrap();
        if first == '!' {
            s = format!("^{}", &s['!'.len_utf8()..]);
        } else if first == '^' || first == '[' {
            s = format!("\\{s}");
        }
        body.push('[');
        body.push_str(&s);
        body.push(']');
    }
    next
}

/// Build the inner text of a `[...]` class: handle ranges, drop empty ranges, escape backslashes
/// and the regex set operators `&`, `~`, `|`. Port of the `stuff = ...` logic in
/// `fnmatch._translate`.
fn build_class_stuff(chars: &[char], i_start: usize, j: usize) -> String {
    let content: String = chars[i_start..j].iter().collect();

    let stuff = if !content.contains('-') {
        content.replace('\\', "\\\\")
    } else {
        let mut chunks: Vec<String> = Vec::new();
        let mut i = i_start;
        let mut k = if chars[i_start] == '!' { i_start + 2 } else { i_start + 1 };
        loop {
            match (k..j).find(|&idx| chars[idx] == '-') {
                None => break,
                Some(pos) => {
                    chunks.push(chars[i..pos].iter().collect());
                    i = pos + 1;
                    k = pos + 3;
                }
            }
        }
        let tail: String = chars[i..j].iter().collect();
        if !tail.is_empty() {
            chunks.push(tail);
        } else if let Some(last) = chunks.last_mut() {
            last.push('-');
        }

        let mut idx = chunks.len();
        while idx > 1 {
            idx -= 1;
            let prev_last = chunks[idx - 1].chars().last();
            let cur_first = chunks[idx].chars().next();
            if let (Some(pl), Some(cf)) = (prev_last, cur_first)
                && pl > cf
            {
                let prev_trimmed: String = {
                    let count = chunks[idx - 1].chars().count();
                    chunks[idx - 1].chars().take(count - 1).collect()
                };
                let cur_rest: String = chunks[idx].chars().skip(1).collect();
                chunks[idx - 1] = prev_trimmed + &cur_rest;
                chunks.remove(idx);
            }
        }

        chunks
            .iter()
            .map(|s| s.replace('\\', "\\\\").replace('-', "\\-"))
            .collect::<Vec<_>>()
            .join("-")
    };

    let mut result = String::new();
    for ch in stuff.chars() {
        if matches!(ch, '&' | '~' | '|') {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}

/// Escape a single character the way Python's `re.escape` does (only ASCII special characters).
fn re_escape_char(c: char) -> String {
    const SPECIAL: &[char] = &[
        '(', ')', '[', ']', '{', '}', '?', '*', '+', '-', '|', '^', '$', '\\', '.', '&', '~',
        '#', ' ', '\t', '\n', '\r', '\u{0b}', '\u{0c}',
    ];
    if SPECIAL.contains(&c) {
        let mut s = String::with_capacity(2);
        s.push('\\');
        s.push(c);
        s
    } else {
        c.to_string()
    }
}

// --- internals: a faithful port of `glob._iglob` and friends ------------------------------
//
// These return lazy iterators (`Box<dyn Iterator>`), mirroring CPython's generator-based `_iglob`
// family: directories are scanned only as results are pulled. Per-directory listings are eager
// (CPython's `_listdir` collects too); the laziness is across directories and `**` recursion.

/// A lazily-produced stream of matched paths.
type PathIter = Box<dyn Iterator<Item = String>>;

/// Port of `glob._iglob`. `root` is the search root (CPython's `root_dir`); the yielded paths are
/// relative to it. `dironly` restricts directory listings to subdirectories.
fn iglob_inner(pathname: &str, root: &str, recursive: bool, dironly: bool, include_hidden: bool) -> PathIter {
    let (dirname, basename) = split(pathname);

    if !has_magic(pathname) {
        let hit = if !basename.is_empty() {
            lexists(&rootjoin(root, pathname))
        } else {
            // Patterns ending in a separator match only directories.
            isdir(&rootjoin(root, &dirname))
        };
        return if hit {
            Box::new(std::iter::once(pathname.to_string()))
        } else {
            Box::new(std::iter::empty())
        };
    }

    if dirname.is_empty() {
        return if recursive && is_recursive(&basename) {
            glob2(root.to_string(), dironly, include_hidden)
        } else {
            Box::new(glob1(root, &basename, dironly, include_hidden).into_iter())
        };
    }

    // Expand the directory part first (it may itself contain magic).
    let dirs: PathIter = if dirname != pathname && has_magic(&dirname) {
        iglob_inner(&dirname, root, recursive, true, include_hidden)
    } else {
        Box::new(std::iter::once(dirname.clone()))
    };

    let recurse_base = recursive && is_recursive(&basename);
    let magic_base = has_magic(&basename);
    let root = root.to_string();
    let basename = basename.clone();
    Box::new(dirs.flat_map(move |d| {
        let scan = rootjoin(&root, &d);
        let names: PathIter = if magic_base {
            if recurse_base {
                glob2(scan, dironly, include_hidden)
            } else {
                Box::new(glob1(&scan, &basename, dironly, include_hidden).into_iter())
            }
        } else {
            Box::new(glob0(&scan, &basename).into_iter())
        };
        names.map(move |name| pathjoin(&d, &name))
    }))
}

/// Port of `glob._glob1`: non-recursively match `pattern` against the entries of `scan`. Eager
/// (one directory), like CPython's `_glob1`.
fn glob1(scan: &str, pattern: &str, dironly: bool, include_hidden: bool) -> Vec<String> {
    let names = listdir(scan, dironly);
    let names: Vec<String> = if include_hidden || ishidden(pattern) {
        names
    } else {
        names.into_iter().filter(|x| !ishidden(x)).collect()
    };
    fnmatch_filter(&names, pattern)
}

/// Port of `glob._glob0`: a literal basename only needs an existence check.
fn glob0(scan: &str, basename: &str) -> Vec<String> {
    if !basename.is_empty() {
        if lexists(&rootjoin(scan, basename)) {
            return vec![basename.to_string()];
        }
    } else if isdir(scan) {
        // Trailing separator: 'q*x/' should match only directories.
        return vec![String::new()];
    }
    Vec::new()
}

/// Port of `glob._glob2`: the `**` recursive wildcard. Yields `""` for the directory itself, then
/// streams the recursive walk.
fn glob2(scan: String, dironly: bool, include_hidden: bool) -> PathIter {
    let head: PathIter = if scan.is_empty() || isdir(&scan) {
        Box::new(std::iter::once(String::new()))
    } else {
        Box::new(std::iter::empty())
    };
    Box::new(head.chain(rlistdir(scan, dironly, include_hidden)))
}

/// Port of `glob._rlistdir`: lazily and recursively yield relative names under `scan`.
fn rlistdir(scan: String, dironly: bool, include_hidden: bool) -> PathIter {
    let names = listdir(&scan, dironly);
    Box::new(
        names
            .into_iter()
            .filter(move |x| include_hidden || !ishidden(x))
            .flat_map(move |x| {
                let path = if scan.is_empty() { x.clone() } else { rootjoin(&scan, &x) };
                let prefix = x.clone();
                std::iter::once(x).chain(
                    rlistdir(path, dironly, include_hidden).map(move |y| pathjoin(&prefix, &y)),
                )
            }),
    )
}

// --- matching --------------------------------------------------------------------------------

/// `fnmatch.filter` with Windows `os.path.normcase` semantics: both pattern and names are
/// case-folded before matching (so globbing is case-insensitive on Windows).
fn fnmatch_filter(names: &[String], pattern: &str) -> Vec<String> {
    let re = match Regex::new(&fnmatch_rs::translate(&normcase(pattern))) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    names
        .iter()
        .filter(|n| re.is_match(&normcase(n)))
        .cloned()
        .collect()
}

#[cfg(windows)]
fn normcase(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

#[cfg(not(windows))]
fn normcase(s: &str) -> String {
    s.to_string()
}

// --- filesystem helpers ----------------------------------------------------------------------

/// Map a logical path to the path used for filesystem access (empty means the current dir).
fn fs_path(p: &str) -> &str {
    if p.is_empty() { "." } else { p }
}

/// List the entries of directory `scan`; when `dironly`, keep only subdirectories. Returns
/// basenames. A non-existent or unreadable directory yields an empty list (like CPython).
fn listdir(scan: &str, dironly: bool) -> Vec<String> {
    let dir = fs_path(scan);
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if dironly {
            // `entry.is_dir()` in CPython follows symlinks; `Path::is_dir` does too.
            if entry.path().is_dir() {
                out.push(name);
            }
        } else {
            out.push(name);
        }
    }
    out
}

/// `os.path.lexists`: true if the path exists, even as a broken symlink.
fn lexists(p: &str) -> bool {
    std::fs::symlink_metadata(fs_path(p)).is_ok()
}

/// `os.path.isdir`: true if the path is a directory (following symlinks).
fn isdir(p: &str) -> bool {
    Path::new(fs_path(p)).is_dir()
}

// --- path string helpers ---------------------------------------------------------------------

/// Split off a Windows drive (`C:`) prefix, like `ntpath.splitdrive`. On non-Windows targets
/// there are no drives (`posixpath.splitdrive` always returns an empty drive).
#[cfg(windows)]
fn splitdrive(p: &str) -> (&str, &str) {
    let bytes = p.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        (&p[..2], &p[2..])
    } else {
        ("", p)
    }
}

#[cfg(not(windows))]
fn splitdrive(p: &str) -> (&str, &str) {
    ("", p)
}

/// Port of `os.path.split` (drive-aware): split `p` into `(head, tail)` at the last separator,
/// stripping trailing separators from `head` unless it is all separators.
fn split(p: &str) -> (String, String) {
    let (drive, rest) = splitdrive(p);
    let chars: Vec<char> = rest.chars().collect();
    let mut i = chars.len();
    while i > 0 && !is_sep(chars[i - 1]) {
        i -= 1;
    }
    let head: String = chars[..i].iter().collect();
    let tail: String = chars[i..].iter().collect();
    let stripped = head.trim_end_matches(is_sep);
    let head = if stripped.is_empty() { head } else { stripped.to_string() };
    (format!("{drive}{head}"), tail)
}

/// `glob._join` / `os.path.join` for two non-drive parts, normalized to `/` separators.
/// (Separators are not part of the faithful result set, so a single canonical form is used.)
fn rootjoin(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    pathjoin(a, b)
}

/// `os.path.join(a, b)`: joins with `/`, preserving an existing trailing separator and adding
/// a trailing separator when `b` is empty (so trailing-slash patterns keep matching directories).
fn pathjoin(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return if a.ends_with(is_sep) { a.to_string() } else { format!("{a}/") };
    }
    if b.starts_with(is_sep) {
        return b.to_string();
    }
    if a.ends_with(is_sep) {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// Path separators for the target, matching `os.path.sep`/`os.path.altsep`: `/` and `\` on
/// Windows, only `/` elsewhere (on POSIX a backslash is an ordinary filename character).
#[cfg(windows)]
fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

#[cfg(not(windows))]
fn is_sep(c: char) -> bool {
    c == '/'
}

fn ishidden(name: &str) -> bool {
    name.starts_with('.')
}

fn is_recursive(pattern: &str) -> bool {
    pattern == "**"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_detection() {
        assert!(has_magic("*.txt"));
        assert!(has_magic("file[0-9]"));
        assert!(has_magic("a?b"));
        assert!(!has_magic("plain.txt"));
        assert!(!has_magic(""));
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("file*.txt"), "file[*].txt");
        assert_eq!(escape("a?b[c"), "a[?]b[[]c");
        assert_eq!(escape("C:/a*b"), "C:/a[*]b"); // drive is left untouched
        assert_eq!(escape("nothing"), "nothing");
    }

    #[test]
    fn path_split() {
        assert_eq!(split("a/b/c"), ("a/b".to_string(), "c".to_string()));
        assert_eq!(split("a/b/"), ("a/b".to_string(), String::new()));
        assert_eq!(split("c"), (String::new(), "c".to_string()));
        assert_eq!(split("/"), ("/".to_string(), String::new()));
        // Drive letters exist only on Windows; on POSIX "C:" is an ordinary name component.
        #[cfg(windows)]
        assert_eq!(split("C:/x"), ("C:/".to_string(), "x".to_string()));
        #[cfg(not(windows))]
        assert_eq!(split("C:/x"), ("C:".to_string(), "x".to_string()));
    }

    #[test]
    fn path_join() {
        assert_eq!(pathjoin("a", "b"), "a/b");
        assert_eq!(pathjoin("a/", "b"), "a/b");
        assert_eq!(pathjoin("a", ""), "a/");
        assert_eq!(pathjoin("", "b"), "b");
    }

    // Exact `glob.translate` output is separator-dependent; pin it on Windows (the dev platform).
    // Cross-platform faithfulness is covered by difftest.py against the live oracle.
    #[cfg(windows)]
    #[test]
    fn translate_matches_cpython() {
        assert_eq!(translate("*.txt", false, false), r"(?s:(?!\.)[^\\/]*\.txt)\Z");
        assert_eq!(translate("**/*.py", true, false), r"(?s:(?:[^\\/.][^\\/]*[\\/])*(?!\.)[^\\/]*\.py)\Z");
        assert_eq!(translate("a?b", false, true), r"(?s:a[^\\/]b)\Z");
        assert_eq!(translate("[a-c]*", false, false), r"(?s:[a-c][^\\/]*)\Z");
    }

    fn unique_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        t ^ C.fetch_add(1, Ordering::Relaxed)
    }

    // Self-contained filesystem test (no Python oracle): exercises the lazy walk, the hidden-file
    // rule, recursion, and that iglob is usable incrementally. Cross-platform.
    #[test]
    fn globs_a_real_tree() {
        use std::fs;
        let root = std::env::temp_dir().join(format!("glob_rs_t_{}_{}", std::process::id(), unique_id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        for f in ["a.txt", "b.txt", "c.py", "sub/d.txt"] {
            fs::write(root.join(f), "x").unwrap();
        }
        fs::write(root.join(".hidden.txt"), "x").unwrap();
        let opts = |recursive, include_hidden| Options {
            recursive,
            include_hidden,
            root_dir: Some(root.to_string_lossy().into_owned()),
        };

        let mut top = glob_with("*.txt", &opts(false, false));
        top.sort();
        assert_eq!(top, vec!["a.txt".to_string(), "b.txt".to_string()]); // hidden excluded

        let mut rec: Vec<String> = glob_with("**/*.txt", &opts(true, false))
            .into_iter()
            .map(|p| p.replace('\\', "/"))
            .collect();
        rec.sort();
        assert!(rec.contains(&"a.txt".to_string()));
        assert!(rec.contains(&"sub/d.txt".to_string()));

        let mut hid = glob_with("*.txt", &opts(false, true));
        hid.sort();
        assert!(hid.contains(&".hidden.txt".to_string()));

        // iglob yields incrementally without collecting the whole tree first.
        assert!(iglob_with("*", &opts(false, false)).next().is_some());

        let _ = fs::remove_dir_all(&root);
    }
}
