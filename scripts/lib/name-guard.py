#!/usr/bin/env python3
"""The tool's old names stay out of the tracked tree.

    name-guard.py [REPO]          (REPO defaults to the current directory)

Reads every tracked path and every tracked file itself (`git ls-files -z`; not `git grep`, whose
exit 128 could be read as clean, and whose -w never matches a prefix followed by a letter) and
fails on substrings, in file contents and in path names alike:
  - the old name of the tool, in any case;
  - the old npm scope, in any case, and the owner's name followed by - + [ % _ or * (the old
    tarball prefix, a pnpm store name, an encoded scope, a regex over them); its URLs, the
    bundle ID, an email address and a home folder (followed by / \\ . @ or a letter) pass;
  - the old two-letter prefix (s and d in any case, then `-` or `_`),
    except in a word listed exactly in REPO/scripts/lib/name-guard.allow (one word per line; a
    line that starts with # is a comment). A listed word that occurs nowhere is a failure too,
    so the list cannot keep a hole open after its word is gone (the list's own lines do not
    count as occurrences). The list is read as staged, and must not be a symlink.
Both the file on disk and its staged content are read (a staged old name over a clean file is a
hit); an allow-list that exists but is not tracked is exit 2. A submodule is not scanned, nor
content that is not ASCII-compatible (UTF-16, compressed). A word is the run of [A-Za-z0-9_.-]
around the match. The patterns are built from pieces, so
this file is scanned like any other. A symlink is scanned by its target text. Untracked files
are not scanned (a fixture must be added to the index first).

Prints each hit as `path:line: word`, then `name-guard: scanned N tracked paths, H hits`.
Exit 0 with no hit; 1 with a hit; 2 when git fails or a tracked file cannot be read.
"""
import os
import re
import subprocess
import sys

OLD = ("sheep" + "dog",)                          # any case
SCOPE = ("@" + "lukaso",)                         # any case, after AT_FORMS turn every spelling of @ into @
# six spellings of the @ besides @ itself: &commat;, &#64; and &#x40; (any leading zeros, with the ;),
# %40, and the backslash-u and backslash-x escapes. Not every spelling: \u{40}, an entity without
# its ;, %2540, octal and the full-width @ pass (the guard catches mistakes, not disguises)
AT_FORMS = re.compile("&commat;|&#0*64;|&#x0*40;|%40|" + re.escape(chr(92)) + "[ux]0*40", re.I)
OWNER = re.compile("lukas" + "o" + r"([-+\[%_*])")  # any case: the old scope and tarball shapes
PREFIX = ("s" + "d_", "s" + "d-")                 # any case
ALLOW = "scripts/lib/name-guard.allow"
WORD = re.compile(r"[A-Za-z0-9_.\-]")


def word_at(text, i, n):
    a, b = i, i + n
    while a > 0 and WORD.match(text[a - 1]):
        a -= 1
    while b < len(text) and WORD.match(text[b]):
        b += 1
    return text[a:b]


def scan(label, text, allow, seen, hits, counts=True):
    low = text.lower()
    for ln, (line, lline) in enumerate(zip(text.split("\n"), low.split("\n")), 1):
        aline = AT_FORMS.sub("@", lline)
        for pat in OLD:
            if pat.lower() in lline:
                hits.append(f"{label}:{ln}: {pat}")
        for pat in SCOPE:
            if pat.lower() in aline:
                hits.append(f"{label}:{ln}: {pat}")
        for m in OWNER.finditer(lline):
            hits.append(f"{label}:{ln}: {m.group(0)}")
        for pat in PREFIX:
            i = lline.find(pat)
            while i >= 0:
                w = word_at(line, i, len(pat))
                if w in allow:
                    if counts:
                        seen.add(w)
                else:
                    hits.append(f"{label}:{ln}: {w}")
                i = lline.find(pat, i + 1)


def main():
    repo = sys.argv[1] if len(sys.argv) > 1 else "."
    def git(*args, data=None):
        return subprocess.run(["git", "-C", repo, *args], input=data, capture_output=True, check=False)
    try:
        out = git("ls-files", "-s", "-z")
    except OSError as e:
        print(f"name-guard: cannot run git: {e}", file=sys.stderr)
        return 2
    if out.returncode != 0:
        print(f"name-guard: git ls-files failed (exit {out.returncode}): {out.stderr.decode(errors='replace').strip()}", file=sys.stderr)
        return 2
    entries = []                                   # (path, blob id or None for a submodule)
    for e in out.stdout.split(b"\0"):
        if not e:
            continue
        meta, _, path = e.partition(b"\t")
        mode, sha = meta.split()[:2]
        entries.append((path.decode("utf-8", "surrogateescape"), None if mode == b"160000" else sha.decode(), mode.decode()))
    paths = [p for p, _, _ in entries]
    # the index's content too: an old name staged over a clean file on disk is a hit
    staged = {}
    shas = [sha for _, sha, _ in entries if sha]
    if shas:
        cat = git("cat-file", "--batch", data="".join(x + "\n" for x in shas).encode())
        if cat.returncode != 0:
            print("name-guard: git cat-file failed", file=sys.stderr)
            return 2
        buf, i = cat.stdout, 0
        for sha in shas:
            head_end = buf.index(b"\n", i)
            head = buf[i:head_end].split()
            if len(head) != 3:                     # `<sha> missing`: an index entry with no object
                miss = [p for p, x, _ in entries if x == sha]
                print(f"name-guard: the staged {miss[0] if miss else sha} is not in the object store", file=sys.stderr)
                return 2
            size = int(head[2])
            staged[sha] = buf[head_end + 1:head_end + 1 + size].decode("latin-1")
            i = head_end + 1 + size + 1
    allow = set()
    # the list as staged (an unstaged word is not part of what is committed); not a symlink
    listed = [(sha, mode) for p, sha, mode in entries if p == ALLOW]
    if listed:
        sha, mode = listed[0]
        if mode == "120000":
            print(f"name-guard: {ALLOW} is a symlink (the list must be a file of its own)", file=sys.stderr)
            return 2
        allow = {l.strip() for l in staged.get(sha, "").split("\n") if l.strip() and not l.startswith("#")}   # "\n" only, as scan(): splitlines() also breaks at 0x85, inside UTF-8 letters
    elif os.path.lexists(os.path.join(repo, ALLOW)):
        print(f"name-guard: {ALLOW} is not tracked (an untracked list would open holes no one reviews)", file=sys.stderr)
        return 2
    seen, hits = set(), []
    for p, sha, _ in entries:
        scan(f"(path) {p}", p, allow, seen, hits)
        full = os.path.join(repo, p)
        try:
            if os.path.islink(full):
                text = os.readlink(full)
            elif sha is None:
                text = ""                          # a submodule: its own repo, not scanned here
            else:
                with open(full, "rb") as f:
                    text = f.read().decode("latin-1")
        except OSError as e:
            print(f"name-guard: cannot read the tracked file {p}: {e}", file=sys.stderr)
            return 2
        scan(p, text, allow, seen, hits, counts=(p != ALLOW))
        if sha and staged.get(sha, text) != text:
            scan(p, staged[sha], allow, seen, hits, counts=(p != ALLOW))
    for w in sorted(allow - seen):
        hits.append(f"{ALLOW}: {w} is listed but occurs nowhere")
    for h in hits:
        print(h)
    print(f"name-guard: scanned {len(paths)} tracked paths, {len(hits)} hits")
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main())
