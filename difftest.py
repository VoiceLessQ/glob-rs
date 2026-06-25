r"""Differential test: glob-rs vs Python's `glob.glob`.

Builds a fixture directory tree, then runs a matrix of (recursive, include_hidden, pattern)
invocations through both CPython's `glob.glob` (the oracle) and this crate (via the `diff`
binary), checking the matched paths agree as sets. Result ordering is unspecified in both, and
path separators are normalized to '/', because neither is part of glob's faithful contract. The
set of matched paths is.

Run from the glob-rs/ folder with the in-tree Python (after `cargo build`):
    ..\..\python\python\python.exe difftest.py
"""

import glob
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
RUST_BIN = os.path.join(HERE, "target", "debug", "diff.exe" if os.name == "nt" else "diff")
SEP = "\x1f"  # unit separator between paths in one result line (never appears in a filename)

# Fixture tree (relative paths; trailing '/' means a directory, others are files).
TREE = [
    "a.txt", "b.txt", "c.py", "ab.txt", "A.TXT", "noext", "a.b.txt", "x.TXT",
    "Mixed.Txt", "file1.log", "file2.log", "fileX.log", "weird[x].txt", ".x",
    ".hidden.txt",
    ".hiddendir/", ".hiddendir/x.txt", ".hiddendir/.deeper_hidden",
    "sub/", "sub/d.txt", "sub/e.py", "sub/.secret", "sub/UPPER.TXT",
    "sub/deep/", "sub/deep/f.txt", "sub/deep/g.md", "sub/deep/.deep_hidden",
    "sub/deep/deeper/", "sub/deep/deeper/h.txt", "sub/deep/deeper/i.py",
    "sub/.hidsub/", "sub/.hidsub/j.txt",
    "sub2/", "sub2/h.txt", "sub2/k.md",
    "Dir/", "Dir/m.txt",
    "empty/",
]

PATTERNS = [
    "*", "*.txt", "*.py", "*.*", "?.txt", "??.txt", "[ab].txt", "[!a]*.txt",
    "[A-Z]*", "[a-c]*", "[!.]*", "file[0-9].log", "file[!0-9].log", "[z-a]*",
    "a.txt", "missing.txt", "missing*", "noext", "a.b.txt", "*b*", "a*", "*.log",
    "weird[[]x].txt",  # escaped literal bracket
    ".*", ".hidden*", ".hiddendir/*", ".hiddendir/.*", ".x",
    "sub", "sub/", "sub/*", "sub/*.py", "sub/*.txt", "sub/d.txt", "sub/missing",
    "sub/.*", "sub/.secret", "sub/deep/*", "sub/deep/*.md", "sub/*/",
    "*/", "*/*", "*/*.txt", "*/*/*.txt", "*/*/*/*.txt", "./*",
    "**", "**/*", "**/*.txt", "**/*.py", "**/*.md", "sub/**", "sub/**/*.txt",
    "**/", "**/deep/*", "**/deeper/*", "sub/**/", "**/*.log",
    "A.TXT", "a.TXT", "*.TXT", "MIXED.TXT", "dir/*", "DIR/*.txt",
    "sub\\d.txt", "sub\\*.txt",  # backslash separator in the pattern
]

FLAGS = [(False, False), (True, False), (False, True), (True, True)]


def build_tree(root):
    for item in TREE:
        path = os.path.join(root, item.replace("/", os.sep))
        if item.endswith("/"):
            os.makedirs(path, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as fh:
                fh.write("x")


def norm_set(paths):
    return {p.replace("\\", "/") for p in paths}


def main():
    if not os.path.exists(RUST_BIN):
        sys.exit(f"missing {RUST_BIN} - run `cargo build` first")

    root = tempfile.mkdtemp(prefix="glob_difftest_")
    try:
        build_tree(root)

        cases = [(r, h, p) for (r, h) in FLAGS for p in PATTERNS]
        lines = [f"{int(r)}\t{int(h)}\t{p}" for (r, h, p) in cases]

        proc = subprocess.run(
            [RUST_BIN, "glob", root],
            input="\n".join(lines),
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        if proc.returncode != 0:
            sys.exit(f"rust diff binary failed:\n{proc.stderr}")

        rust_lines = proc.stdout.split("\n")
        if len(rust_lines) < len(cases):
            sys.exit(f"line count mismatch: {len(rust_lines)} rust vs {len(cases)} cases")

        mismatches = []
        for (r, h, p), rline in zip(cases, rust_lines):
            expected = norm_set(glob.glob(p, root_dir=root, recursive=r, include_hidden=h))
            got = norm_set(x for x in rline.split(SEP) if x != "")
            if expected != got:
                mismatches.append((r, h, p, expected, got))

        if mismatches:
            print(f"{len(mismatches)} mismatches (of {len(cases)}):")
            for r, h, p, e, g in mismatches[:40]:
                print(f"  recursive={r} hidden={h} pattern={p!r}")
                print(f"      python-only: {sorted(e - g)}")
                print(f"      rust-only:   {sorted(g - e)}")
            sys.exit("\nMISMATCHES FOUND.")

        print(f"ALL MATCH - {len(cases)} glob invocations agree with Python glob.")

        check_translate()
    finally:
        shutil.rmtree(root, ignore_errors=True)


# Patterns for translate(): the FS patterns plus separator-heavy and edge-class cases.
TRANSLATE_PATTERNS = PATTERNS + [
    "a//b", "a/", "/a", "", "**/**", "a/**/b", "**/**/*.txt", "x/**/**/y",
    "[]", "[!]", "[]]", "[!]]", "[a-c]", "[z-a]", "[^x]", "[[]", "[&~|]",
    "[abc-]", "[-abc]", "a.b.c", "*?*", "?*?", "**.txt",
]


def check_translate():
    cases = [(r, h, p) for (r, h) in FLAGS for p in TRANSLATE_PATTERNS]
    lines = [f"{int(r)}\t{int(h)}\t{p}" for (r, h, p) in cases]

    proc = subprocess.run(
        [RUST_BIN, "translate"],
        input="\n".join(lines),
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if proc.returncode != 0:
        sys.exit(f"rust translate failed:\n{proc.stderr}")

    rust_lines = proc.stdout.split("\n")
    if len(rust_lines) < len(cases):
        sys.exit(f"translate line count mismatch: {len(rust_lines)} vs {len(cases)}")

    mismatches = []
    for (r, h, p), got in zip(cases, rust_lines):
        expected = glob.translate(p, recursive=r, include_hidden=h)
        if expected != got:
            mismatches.append((r, h, p, expected, got))

    if mismatches:
        print(f"\n{len(mismatches)} translate mismatches (of {len(cases)}):")
        for r, h, p, e, g in mismatches[:40]:
            print(f"  recursive={r} hidden={h} pattern={p!r}")
            print(f"      python: {e!r}")
            print(f"      rust:   {g!r}")
        sys.exit("\nTRANSLATE MISMATCHES FOUND.")

    print(f"ALL MATCH - {len(cases)} translate() outputs byte-identical to Python glob.translate.")


if __name__ == "__main__":
    main()
