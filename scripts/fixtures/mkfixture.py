#!/usr/bin/env python3
"""Helpers for scripts/fixtures/build.sh (stdlib only).

  mkfixture.py stage <src> <dst> [hooks.wasm]   copy a fixture source dir, add icon/guide/wasm
  mkfixture.py mutate <archive> <mode>          damage a built archive (archive-level invalid cases)
"""
import gzip, io, os, shutil, struct, sys, tarfile, zlib

SKIP = {"hooks", "mutate", "target", "dist"}


def png(w, h):
    # Header-only PNG, the same stub the registry's PackageBuilder uses.
    return (b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR"
            + struct.pack(">II", w, h) + bytes([8, 6, 0, 0, 0, 0, 0, 0, 0]))


def read_mutation(src):
    p = os.path.join(src, "mutate")
    return open(p).read().strip() if os.path.isfile(p) else ""


def stage(src, dst, wasm=None):
    if os.path.exists(dst):
        shutil.rmtree(dst)
    os.makedirs(dst)
    for name in sorted(os.listdir(src)):
        if name in SKIP:
            continue
        s = os.path.join(src, name)
        if os.path.isdir(s):
            shutil.copytree(s, os.path.join(dst, name))
        else:
            shutil.copy(s, os.path.join(dst, name))
    os.makedirs(os.path.join(dst, "assets"), exist_ok=True)
    os.makedirs(os.path.join(dst, "docs"), exist_ok=True)
    mode = read_mutation(src)
    size = 32 if mode == "icon32" else 64
    with open(os.path.join(dst, "assets/icon.png"), "wb") as f:
        f.write(png(size, size))
    guide = os.path.join(dst, "docs/setup.md")
    if mode == "guide-html":
        open(guide, "w").write("# Setup\n\n<script>alert(1)</script>\n")
    elif not os.path.exists(guide):
        open(guide, "w").write("# Setup\n\nTest fixture package.\n")
    if wasm:
        shutil.copy(wasm, os.path.join(dst, "hooks.wasm"))


def rewrite(path, fn):
    raw = gzip.decompress(open(path, "rb").read())
    src = tarfile.open(fileobj=io.BytesIO(raw))
    out = io.BytesIO()
    dst = tarfile.open(fileobj=out, mode="w", format=tarfile.GNU_FORMAT)
    for m in src.getmembers():
        data = src.extractfile(m).read() if m.isreg() else None
        fn(dst, m, data)
    fn(dst, None, None)
    dst.close()
    open(path, "wb").write(gzip.compress(out.getvalue()))


def mutate(path, mode):
    def add(dst, name, data=b"x", typ=tarfile.REGTYPE, link=""):
        ti = tarfile.TarInfo(name)
        ti.type, ti.size, ti.linkname, ti.mode = typ, len(data) if typ == tarfile.REGTYPE else 0, link, 0o644
        dst.addfile(ti, io.BytesIO(data) if typ == tarfile.REGTYPE else None)

    def keep(dst, m, data):
        if m is not None:
            ti = tarfile.TarInfo(m.name)
            ti.size, ti.mode = m.size, m.mode
            dst.addfile(ti, io.BytesIO(data))

    if mode == "tamper":
        def fn(dst, m, data):
            if m is not None and m.name == "plugin.yaml":
                data = data.replace(b"name: Invalid fixture", b"name: Tampered fixture", 1)
                m.size = len(data)
            keep(dst, m, data)
    elif mode == "drop-wasm":
        def fn(dst, m, data):
            if m is not None and m.name != "hooks.wasm":
                keep(dst, m, data)
    elif mode == "symlink":
        def fn(dst, m, data):
            keep(dst, m, data)
            if m is None:
                add(dst, "docs/link", typ=tarfile.SYMTYPE, link="/etc/passwd")
    elif mode == "traversal":
        def fn(dst, m, data):
            keep(dst, m, data)
            if m is None:
                add(dst, "docs/../../escape.txt")
    elif mode == "unknown-path":
        def fn(dst, m, data):
            keep(dst, m, data)
            if m is None:
                add(dst, "extra/payload.bin")
    elif mode == "duplicate":
        def fn(dst, m, data):
            keep(dst, m, data)
            if m is not None and m.name == "docs/setup.md":
                keep(dst, m, data)
    else:
        sys.exit(f"unknown mutation {mode}")
    rewrite(path, fn)


if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["stage"]:
        stage(a[1], a[2], a[3] if len(a) > 3 else None)
    elif a[:1] == ["mutate"]:
        mutate(a[1], a[2])
    else:
        sys.exit(__doc__)
