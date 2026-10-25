"""Dump ETSI spec tables with word coordinates, so multi-row tables can be
reconstructed without the column scrambling that plain text extraction causes.

Usage: python -I spec_table_dump.py <pdf> <first-page-0based> <last-page-0based> <out.txt>
"""

import io
import sys

import pypdf


def dump(path, first, last, out_path):
    reader = pypdf.PdfReader(path)
    out = io.open(out_path, "w", encoding="utf-8")
    for pno in range(first, last + 1):
        page = reader.pages[pno]
        items = []

        def visit(text, _cm, tm, _font, _size):
            t = text.strip()
            if t:
                items.append((round(tm[5], 1), round(tm[4], 1), t))

        page.extract_text(visitor_text=visit)
        out.write("#### page index %d\n" % pno)
        # group by y (rounded to 2 pt), descending
        rows = {}
        for y, x, t in items:
            rows.setdefault(round(y / 2.0) * 2.0, []).append((x, t))
        for y in sorted(rows, reverse=True):
            cells = sorted(rows[y])
            out.write("y=%7.1f | " % y)
            out.write("  ".join("%s@%.0f" % (t, x) for x, t in cells))
            out.write("\n")
        out.write("\n")
    out.close()


if __name__ == "__main__":
    dump(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4])
