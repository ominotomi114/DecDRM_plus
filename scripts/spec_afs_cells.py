"""Extract ES 201 980 table 61 (AFS reference cells, robustness mode E).

Each row is one text object of the form "<k> <phase s=4> <phase s=39>", but two things
break naive splitting:

  * phases of 1000 or more are printed with a space as a thousands separator ("1 001");
  * an asterisk may follow either phase, marking a cell that is *both* an AFS and a gain
    reference (the note under the table says the phase definition is the AFS one and the
    amplitude is the gain reference's).

So the row is parsed by scanning tokens and greedily joining a leading "1" with a
following three-digit group.

Prints the two symbol tables as Rust constants and their counts, so the counts can be
checked against the spec's own NMUX/NSDC arithmetic.
"""

import re
import sys

import pypdf

ROW = re.compile(r"^(-?\d+) (.+)$")


def parse_row(text):
    """Return (k, phase_s4, star4, phase_s39, star39) or None."""
    m = ROW.match(text.strip())
    if not m:
        return None
    k = int(m.group(1))
    toks = m.group(2).split()
    phases = []
    i = 0
    while i < len(toks):
        t = toks[i]
        # A leading "1" followed by a bare three-digit group is a thousands separator.
        if t == "1" and i + 1 < len(toks) and re.fullmatch(r"\d{3}\*?", toks[i + 1]):
            t = "1" + toks[i + 1]
            i += 1
        star = t.endswith("*")
        num = t.rstrip("*")
        if not re.fullmatch(r"\d+", num):
            return None
        phases.append((int(num), star))
        i += 1
    if len(phases) != 2:
        return None
    return (k, phases[0][0], phases[0][1], phases[1][0], phases[1][1])


def page_texts(page):
    out = []

    def visit(text, _cm, tm, _font, _size):
        t = text.strip()
        if t:
            out.append((round(tm[5], 1), t))

    page.extract_text(visitor_text=visit)
    return out


def main():
    path = sys.argv[1]
    reader = pypdf.PdfReader(path)
    table = {}
    for pno in (125, 126):
        for y, t in page_texts(reader.pages[pno]):
            row = parse_row(t)
            if row:
                table.setdefault(row[0], row[1:])

    ks = sorted(table)
    print("carriers found: %d" % len(ks))
    missing = [k for k in range(-106, 107) if k not in table]
    print("missing carriers: %d %s" % (len(missing), missing[:10]))

    for name, off in (("AFS_S4", 0), ("AFS_S39", 2)):
        vals = [(k, table[k][off], table[k][off + 1]) for k in ks]
        starred = sum(1 for v in vals if v[2])
        print()
        print("%s: %d entries, %d starred" % (name, len(vals), starred))
        print("pub const %s: [(i16, u16); %d] = [" % (name, len(vals)))
        line = "   "
        for k, n, star in vals:
            tok = " (%d, %d)," % (k, n)
            if len(line) + len(tok) > 96:
                print(line)
                line = "   "
            line += tok
        print(line)
        print("];")


if __name__ == "__main__":
    main()

