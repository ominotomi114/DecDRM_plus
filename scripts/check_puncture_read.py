"""Cross-check the puncturing patterns read out of ES 201 980 table 27 against the
13 rates DecDRM already implements, so the reading method is known to be sound
before it is used for the rates DecDRM lacks (1/6, 2/5).

Prints, for each rate, the pattern implied by `(RX, RY)` and the number of output
bits per period, so the two can be compared by eye and by assertion.
"""

# (name, RX, RY, pattern) exactly as in crates/decdrm-core/src/tables.rs CODE_RATES.
BERS = {
    "P1111": 0b1111,
    "P0111": 0b0111,
    "P0011": 0b0011,
    "P0001": 0b0001,
    "P0101": 0b0101,
}

EXISTING = [
    ("1/4", 1, 4, ["P1111"]),
    ("3/10", 3, 10, ["P1111", "P0111", "P0111"]),
    ("1/3", 1, 3, ["P0111"]),
    ("4/11", 4, 11, ["P0111", "P0111", "P0111", "P0011"]),
    ("1/2", 1, 2, ["P0011"]),
    ("4/7", 4, 7, ["P0011", "P0101", "P0011", "P0001"]),
    ("3/5", 3, 5, ["P0011", "P0001", "P0011"]),
    ("2/3", 2, 3, ["P0011", "P0001"]),
    ("8/11", 8, 11, ["P0011", "P0001", "P0001", "P0011", "P0001", "P0001", "P0011", "P0001"]),
    ("3/4", 3, 4, ["P0011", "P0001", "P0001"]),
    ("4/5", 4, 5, ["P0011", "P0001", "P0001", "P0001"]),
    ("7/8", 7, 8, ["P0011", "P0001", "P0001", "P0001", "P0001", "P0001", "P0001"]),
    ("8/9", 8, 9, ["P0011", "P0001", "P0001", "P0001", "P0001", "P0001", "P0001", "P0001"]),
]


def ones_per_period(pattern):
    return sum(bin(BERS[p]).count("1") for p in pattern)


print("rate   RX RY  outputs/period  RX/RY  implied rate  pattern")
for name, rx, ry, pat in EXISTING:
    out = ones_per_period(pat)
    ok = "OK" if out == ry and len(pat) == rx else "MISMATCH"
    print(
        "%-6s %2d %2d  %2d              %.4f  %.4f  %-40s %s"
        % (name, rx, ry, out, rx / ry, out / len(pat) if pat else 0, "".join(pat), ok)
    )

# The two rates DecDRM lacks, as read from table 27 with word coordinates.
# 1/6  : B0..B5 each a single "1"  -> mask 0b111111 (all six mother outputs kept)
# 2/5  : row y=590 on page 100 gave  B0: 1 1 / B1: 1 1 / B2: 1 0 / B3..B5: 0 0
NEW = [
    ("1/6", 1, 6, [0b111111]),
    ("2/5", 2, 5, [0b110001, 0b110001]),
]
print()
for name, rx, ry, pat in NEW:
    out = sum(bin(p).count("1") for p in pat)
    print("%-6s %2d %2d  %2d              %.4f  %.4f" % (name, rx, ry, out, rx / ry, out / len(pat)))
