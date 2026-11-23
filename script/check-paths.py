#!/usr/bin/env python3
"""Fails when a doc or a comment names a file that isn't in the repository.

What it reads:
  - CLAUDE.md, README.md, CONTRIBUTING.md and every *.md under docs/: each
    path in backticks, and each relative Markdown link.
  - Comments in *.rs and *.swift files: each mention shaped like a path
    (`dir/file.rs`, `Model/AppModel.swift`, `crates/x/src/y.rs:120`), and
    each bare `file.rs` or `file.swift`.

A mention resolves when a tracked file (or folder, for one ending in `/`)
is that path, or ends with it after a `/`, so a path given relative to its
crate's `src/` or the iOS app's folder counts, and so does a crate named
without its `deptheos_` (`gpui/src/app.rs`). Braces expand
(`{pyramid,depth}/mod.rs`); a line suffix (`:120`, `:12-30`) is ignored;
anything with `<`, `>`, `*`, `$`, `%` or `…` in it is a pattern, not a
path.

script/check-paths.allow lists the mentions that are meant not to resolve
(generated files, paths in a Library or on a user's disk), one per line,
`#` for comments.

Run from the repository root: `python3 script/check-paths.py`.
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

EXTENSIONS = (
    "rs swift md toml json wgsl sh ts mjs js html css cpp h hpp c mm m yml "
    "yaml ftl snap plist xcconfig ps1 iss py txt tsv entitlements xcprivacy "
    "xcodeproj xcframework metal"
).split()
EXT = "|".join(EXTENSIONS)

# A path in a comment: at least one `/`, ending in a known extension.
COMMENT_PATH = re.compile(
    r"(?<![\w./{}+-])((?:[\w.{},+-]+/)+[\w.{},+-]+\.(?:" + EXT + r"))(?::[\d-]+)?(?![\w/])"
)
# A bare Rust or Swift file name in a comment.
BARE_FILE = re.compile(r"(?<![\w./{}+-])([A-Za-z_][\w+-]*\.(?:rs|swift))(?![\w/])")
BACKTICK = re.compile(r"`([^`\n]+)`")
MD_LINK = re.compile(r"\]\(([^)\s]+)\)")


def tracked():
    out = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split("\n")
    files = [f for f in out if f]
    dirs = set()
    for f in files:
        parts = f.split("/")
        for i in range(1, len(parts)):
            dirs.add("/".join(parts[:i]))
    return files, dirs


FILES, DIRS = tracked()
SUFFIXES = set()
BASENAMES = set()
for path in list(FILES) + list(DIRS):
    parts = path.split("/")
    for i in range(len(parts)):
        SUFFIXES.add("/".join(parts[i:]))
    BASENAMES.add(parts[-1])


def load_allow():
    allowed = set()
    try:
        with open("script/check-paths.allow") as f:
            for line in f:
                line = line.split("#", 1)[0].strip()
                if line:
                    allowed.add(line)
    except FileNotFoundError:
        pass
    return allowed


ALLOW = load_allow()


def expand(text):
    m = re.search(r"\{([^{}]*)\}", text)
    if not m:
        return [text]
    out = []
    for choice in m.group(1).split(","):
        out.extend(expand(text[: m.start()] + choice + text[m.end() :]))
    return out


def resolves(mention, base_dir=None):
    mention = re.sub(r":[\d-]+$", "", mention)
    if mention in ALLOW:
        return True
    for one in expand(mention):
        one = one.rstrip("/")
        if one.startswith("./"):
            one = one[2:]
        if not one:
            continue
        if base_dir is not None:
            joined = os.path.normpath(os.path.join(base_dir, one))
            if joined in FILES or joined in DIRS:
                continue
        if one.startswith("/") or one.startswith("~"):
            continue
        if "/" not in one:
            if one in BASENAMES:
                continue
            return False
        if one in SUFFIXES:
            continue
        # `gpui/src/app.rs` for `crates/deptheos_gpui/src/app.rs`.
        head, _, rest = one.partition("/")
        if f"deptheos_{head}/{rest}" in SUFFIXES:
            continue
        return False
    return True


def is_pattern(text):
    return any(c in text for c in "<>*$…%") or text.startswith("http")


def looks_like_path(text):
    """A backticked span in a doc that is meant as a repository path."""
    text = re.sub(r":[\d-]+$", "", text.strip())
    if " " in text or is_pattern(text) or "/" not in text:
        return False
    if "://" in text or text.startswith(("/", "~", "-", "@")):
        return False
    if text.endswith("/"):
        return bool(re.fullmatch(r"[\w.{},/-]+/", text)) and text.split("/")[0] in TOP
    return bool(re.fullmatch(r"[\w.{},/+-]+\.(?:" + EXT + r")", text))


TOP = {f.split("/")[0] for f in FILES}

problems = []


def report(path, line_no, mention):
    if mention in ALLOW:
        return
    problems.append(f"{path}:{line_no}: names `{mention}`, which isn't in the repository")


def check_doc(path):
    base = os.path.dirname(path)
    with open(path, encoding="utf-8") as f:
        in_code = False
        for n, line in enumerate(f, 1):
            if line.lstrip().startswith("```"):
                in_code = not in_code
                continue
            if in_code:
                continue
            for m in BACKTICK.finditer(line):
                text = m.group(1).strip()
                if looks_like_path(text) and not resolves(text):
                    report(path, n, text)
            for m in MD_LINK.finditer(line):
                target = m.group(1).split("#", 1)[0]
                if not target or is_pattern(target) or ":" in target:
                    continue
                if target.startswith("/"):
                    continue
                joined = os.path.normpath(os.path.join(base, target))
                if joined not in FILES and joined not in DIRS and target not in ALLOW:
                    report(path, n, target)


COMMENT = re.compile(r"//+!?\s?(.*)$")


def check_source(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            m = COMMENT.search(line)
            if not m:
                continue
            # Skip `//` inside a string literal, such as a URL.
            before = line[: m.start()]
            if before.count('"') % 2 == 1:
                continue
            text = m.group(1)
            if "://" in text:
                text = re.sub(r"\S+://\S+", "", text)
            seen = set()
            for pm in COMMENT_PATH.finditer(text):
                mention = pm.group(1)
                seen.add(pm.start(1))
                if is_pattern(mention) or mention.startswith(("~", "/")):
                    continue
                if not resolves(mention):
                    report(path, n, mention)
            for bm in BARE_FILE.finditer(text):
                if bm.start(1) in seen:
                    continue
                mention = bm.group(1)
                if not resolves(mention):
                    report(path, n, mention)


SKIP_DIRS = ("target/", "vendor/", "node_modules/", "native/vendor/", "LibRaw/")

for f in FILES:
    if any(part in f for part in SKIP_DIRS) or not os.path.isfile(f):
        continue
    if f in ("CLAUDE.md", "README.md", "CONTRIBUTING.md") or (
        f.startswith("docs/") and f.endswith(".md")
    ):
        check_doc(f)
    elif f.endswith((".rs", ".swift")):
        check_source(f)

if problems:
    print("\n".join(problems))
    print(
        f"\n{len(problems)} mention(s) of files that don't exist. Fix the comment or doc, "
        "or list a mention meant not to resolve in script/check-paths.allow."
    )
    sys.exit(1)
