#!/usr/bin/env python3
"""Fail when a macOS binary imports an Apple framework symbol newer than its deployment target.

Rust `extern` statics and functions are strong imports: one that the running macOS lacks makes dyld
refuse to start the whole app ("Symbol not found"), before `main` (#330: a macOS 13 VideoToolbox
constant stopped 0.4.0 from launching on Monterey). This lists the binary's imports from the
system frameworks (`nm -um`), looks each one up in the SDK headers, and reports every symbol whose
`API_AVAILABLE(macos(X))` is newer than the minimum (default: the binary's own `minos`).

Usage: packaging/macos/check_symbols.py target/release/filmcraft [--min 11.0]
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

FRAMEWORK = re.compile(r"\(from (\w+)\)$")
AVAILABLE = re.compile(r"(?:API_AVAILABLE|API_DEPRECATED[^(]*)\([^;]*?macos(?:x)?\((\d+)\.(\d+)")
OLD_MACRO = re.compile(r"__OSX_AVAILABLE(?:_STARTING)?\(__MAC_(\d+)_(\d+)")


def version(s):
    parts = [int(p) for p in s.split(".")]
    return tuple(parts + [0] * (2 - len(parts)))[:2]


def binary_minos(path):
    out = subprocess.run(["vtool", "-show-build", path], capture_output=True, text=True).stdout
    m = re.search(r"minos (\d+(?:\.\d+)*)", out)
    return version(m.group(1)) if m else (11, 0)


def imports(path):
    out = subprocess.run(["nm", "-um", path], capture_output=True, text=True, check=True).stdout
    for line in out.splitlines():
        m = FRAMEWORK.search(line.strip())
        if not m or m.group(1) in ("libSystem", "libobjc", "libc++", "libiconv"):
            continue
        sym = line.split()[-3]
        yield sym.lstrip("_"), m.group(1)


def header_text(sdk, framework, cache={}):
    if framework not in cache:
        text = []
        for d in (sdk / "System/Library/Frameworks").glob(f"{framework}.framework/**/Headers"):
            for h in d.glob("*.h"):
                text.append(h.read_text(errors="replace"))
        cache[framework] = "\n".join(text)
    return cache[framework]


def availability(sdk, framework, sym):
    """The macOS version the SDK headers give for `sym`, or None when they don't say."""
    text = header_text(sdk, framework)
    if not text:
        # e.g. CGDisplayCreateUUIDFromDisplayID is linked from ColorSync but declared elsewhere
        text = header_text(sdk, "ApplicationServices") + header_text(sdk, "CoreGraphics")
    for m in re.finditer(r"\b" + re.escape(sym) + r"\b", text):
        # the declaration's attributes follow it up to the end of the statement
        tail = text[m.end(): m.end() + 400].split(";")[0]
        head = text[max(0, m.start() - 300): m.start()].split(";")[-1]
        for chunk in (tail, head):
            a = AVAILABLE.search(chunk) or OLD_MACRO.search(chunk)
            if a:
                return int(a.group(1)), int(a.group(2))
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("binary")
    ap.add_argument("--min", help="minimum macOS (default: the binary's minos)")
    args = ap.parse_args()
    sdk = Path(subprocess.run(["xcrun", "--show-sdk-path"], capture_output=True, text=True, check=True).stdout.strip())
    minimum = version(args.min) if args.min else binary_minos(args.binary)
    newer, unknown, checked = [], 0, 0
    for sym, fw in sorted(set(imports(args.binary))):
        if sym.startswith("OBJC_CLASS_$_"):
            continue  # classes are looked up at run time by objc2
        checked += 1
        v = availability(sdk, fw, sym)
        if v is None:
            unknown += 1
        elif v > minimum:
            newer.append((sym, fw, v))
    print(f"{checked} framework imports checked against macOS {minimum[0]}.{minimum[1]} ({unknown} without an availability attribute)")
    for sym, fw, v in newer:
        print(f"error: {sym} ({fw}) needs macOS {v[0]}.{v[1]}; dyld will refuse to start the app on older systems", file=sys.stderr)
    return 1 if newer else 0


if __name__ == "__main__":
    sys.exit(main())
