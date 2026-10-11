#!/usr/bin/env python3
"""Convert a WOFF1 font to TTF/OTF (sfnt). HarfBuzz (used by Pango) can't read WOFF."""
import struct, sys, zlib

def convert(src, dst):
    data = open(src, "rb").read()
    sig, flavor, length, num_tables = struct.unpack(">4sIIH", data[:14])
    assert sig == b"wOFF", "not a WOFF1 file"
    tables = []
    for i in range(num_tables):
        tag, off, comp_len, orig_len, checksum = struct.unpack(">4sIIII", data[44 + i * 20: 64 + i * 20])
        raw = data[off: off + comp_len]
        tables.append((tag, checksum, zlib.decompress(raw) if comp_len < orig_len else raw))
    tables.sort(key=lambda t: t[0])
    entry_selector = max(i for i in range(16) if 2 ** i <= num_tables)
    search_range = 2 ** entry_selector * 16
    out = bytearray(struct.pack(">IHHHH", flavor, num_tables, search_range, entry_selector,
                                num_tables * 16 - search_range))
    offset = 12 + 16 * num_tables
    body = bytearray()
    for tag, checksum, table in tables:
        out += struct.pack(">4sIII", tag, checksum, offset + len(body), len(table))
        body += table + b"\0" * (-len(table) % 4)
    open(dst, "wb").write(bytes(out + body))

if __name__ == "__main__":
    convert(sys.argv[1], sys.argv[2])
