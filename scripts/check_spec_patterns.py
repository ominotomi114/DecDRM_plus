"""Validate ES 201 980 table 27 (puncturing patterns) as read from the spec PDF.

Plain text extraction scrambles this table: the pattern rows sit in a separate
column from the rate labels, so the grouping between a rate and its B0..B5 rows is
lost, and the rows themselves are the *transpose* of the masks (B_j is one mother
output across the RX input bits of the period). This reads the words with their
coordinates, regroups the rows, and rebuilds each rate's masks.

It then compares them with the 13 rates DecDRM already implements. Reproducing all
13 is what makes the extraction trustworthy for the two rates DecDRM lacks (1/6 and
2/5), and it also shows whether any rate needs the mother outputs b4/b5 — the
question that decides whether DecDRM's four-generator encoder is enough.

`crates/decdrm-core/src/tables.rs` stays the source of truth; the 13 patterns below
are a copy used only as a cross-check.

Usage: python -I check_spec_patterns.py <spec.pdf> [first-page-0based last-page-0based]
"""

import re
import sys

import pypdf

# name -> (RX, RY, masks) copied from tables.rs CODE_RATES.
EXISTING = {
    "1/4": (1, 4, [0b1111]),
    "3/10": (3, 10, [0b1111, 0b0111, 0b0111]),
    "1/3": (1, 3, [0b0111]),
    "4/11": (4, 11, [0b0111, 0b0111, 0b0111, 0b0011]),
    "1/2": (1, 2, [0b0011]),
    "4/7": (4, 7, [0b0011, 0b0101, 0b0011, 0b0001]),
    "3/5": (3, 5, [0b0011, 0b0001, 0b0011]),
    "2/3": (2, 3, [0b0011, 0b0001]),
    "8/11": (8, 11, [0b0011, 0b0001, 0b0001, 0b0011, 0b0001, 0b0001, 0b0011, 0b0001]),
    "3/4": (3, 4, [0b0011, 0b0001, 0b0001]),
    "4/5": (4, 5, [0b0011, 0b0001, 0b0001, 0b0001]),
    "7/8": (7, 8, [0b0011] + [0b0001] * 6),
    "8/9": (8, 9, [0b0011] + [0b0001] * 7),
}

RATE_RE = re.compile(r"\s*(\d+/\d+)\s+(\d+)\s+(\d+)\b")
ROW_RE = re.compile(r"\bB([0-5]):\s*((?:[01]\s*)+)")


def page_rows(page):
    """Return the page's text rows, each a single string in reading order."""
    words = []

    def visit(text, _cm, tm, _font, _size):
        t = text.strip()
        if t:
            words.append((tm[5], tm[4], t))

    page.extract_text(visitor_text=visit)

    by_row = {}
    for y, x, t in words:
        by_row.setdefault(round(y / 2.0), []).append((x, t))
    rows = []
    for y in sorted(by_row, reverse=True):
        rows.append(" ".join(t for _x, t in sorted(by_row[y])))
    return rows


def read_table27(path, first, last):
    reader = pypdf.PdfReader(path)
    rates = {}  # name -> {"rx":, "ry":, "B": {j: [bits]}}
    cur = None
    for pno in range(first, last + 1):
        for row in page_rows(reader.pages[pno]):
            m = RATE_RE.match(row)
            if m:
                cur = {"rx": int(m.group(2)), "ry": int(m.group(3)), "B": {}}
                rates[m.group(1)] = cur
                # The rate label shares its row with B0, e.g. "1/4 1 4 B0: 1", so the
                # B0 row must be picked up here rather than only on the rows below.
            elif cur is None:
                continue
            for rm in ROW_RE.finditer(row):
                cur["B"][int(rm.group(1))] = [int(b) for b in rm.group(2).split()]
    return rates


def masks_of(entry):
    """Transpose the B rows into one mask per input bit: mask[i] = Σ_j B_j[i] << j."""
    rx = entry["rx"]
    masks = []
    for i in range(rx):
        m = 0
        for j in range(6):
            bits = entry["B"].get(j)
            if bits and i < len(bits) and bits[i]:
                m |= 1 << j
        masks.append(m)
    return masks


def main():
    path = sys.argv[1]
    first = int(sys.argv[2]) if len(sys.argv) > 2 else 99
    last = int(sys.argv[3]) if len(sys.argv) > 3 else 101
    rates = read_table27(path, first, last)
    print("read %d rates: %s" % (len(rates), " ".join(rates)))

    bad = 0
    uses_b45 = []
    for name, (rx, ry, masks) in EXISTING.items():
        e = rates.get(name)
        if e is None:
            print("%-6s MISSING from spec read" % name)
            bad += 1
            continue
        got = masks_of(e)
        outs = sum(bin(m).count("1") for m in got)
        ok = got == masks and outs == ry
        if e["B"].get(4) or e["B"].get(5):
            uses_b45.append(name)
        print(
            "%-6s spec %-34s code %-34s outs %2d/%2d  %s"
            % (
                name,
                "[" + ",".join("%06d" % int(bin(m)[2:]) for m in got) + "]",
                "[" + ",".join("%06d" % int(bin(m)[2:]) for m in masks) + "]",
                outs,
                ry,
                "OK" if ok else "MISMATCH",
            )
        )
        if not ok:
            bad += 1

    print()
    print("rates needing mother outputs b4/b5 (among modes A-D rates): %s"
          % (uses_b45 if uses_b45 else "none"))

    print()
    print("rates in the spec but not in CODE_RATES:")
    for name, e in rates.items():
        if name not in EXISTING:
            got = masks_of(e)
            outs = sum(bin(m).count("1") for m in got)
            print(
                "  %-6s RX=%d RY=%d  masks=%s  outs=%d  b4/b5 used: %s"
                % (
                    name,
                    e["rx"],
                    e["ry"],
                    [format(m, "06b") for m in got],
                    outs,
                    bool(e["B"].get(4) or e["B"].get(5)),
                )
            )

    print()
    print("RESULT:", "all 13 existing rates reproduced" if bad == 0 else "%d mismatches" % bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
