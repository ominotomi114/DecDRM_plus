"""Verify the mode E code-rate tables (ES 201 980 tables 29 and 31) against the
standard's own capacity annex (table J.5), which states the number of MSC input bits
per multiplex frame for every mode E rate.

This is the strongest check available: if the rates, the level split and the N2
formula all agree with J.5's published numbers, the table reading is right. It also
confirms NMUX = 7460 for mode E occupancy 0 (table 45).

L2 = Σ_p RX_p · floor((2·N2 − 12) / RY_p)   (clause 7.2.1.1, EEP so N2 = NMUX)
"""

NMUX_E = 7460  # table 45, robustness mode E, spectrum occupancy 0
NSDC_E = 936   # solved below from J.10

# (label, [(RX, RY) per level], expected L from table J.5)
MODE_E = [
    ("16-QAM Rall=0.33", [(1, 6), (1, 2)], 9938),
    ("16-QAM Rall=0.41", [(1, 4), (4, 7)], 12243),
    ("16-QAM Rall=0.5",  [(1, 3), (2, 3)], 14907),
    ("16-QAM Rall=0.62", [(1, 2), (3, 4)], 18635),
    ("4-QAM Rall=0.25",  [(1, 4)], 3727),
    ("4-QAM Rall=0.33",  [(1, 3)], 4969),
    ("4-QAM Rall=0.4",   [(2, 5)], 5962),
    ("4-QAM Rall=0.5",   [(1, 2)], 7454),
]


def l2(levels, n2):
    total = 0
    for rx, ry in levels:
        total += rx * ((2 * n2 - 12) // ry)
    return total


print("MSC (annex J.5, NMUX = %d)" % NMUX_E)
bad = 0
for label, lv, want in MODE_E:
    got = l2(lv, NMUX_E)
    ok = got == want
    bad += not ok
    print("  %-18s computed %6d  spec %6d  %s" % (label, got, want, "OK" if ok else "MISMATCH"))

print()
print("SDC (annex J.10): NSDC = %d" % NSDC_E)
for label, lv, want in [("4-QAM Rall=0.5", [(1, 2)], 930), ("4-QAM Rall=0.25", [(1, 4)], 465)]:
    got = l2(lv, NSDC_E)
    ok = got == want
    bad += not ok
    print("  %-18s computed %6d  spec %6d  %s" % (label, got, want, "OK" if ok else "MISMATCH"))

print()
print("RESULT:", "all mode E rates reproduce annex J" if bad == 0 else "%d mismatches" % bad)
