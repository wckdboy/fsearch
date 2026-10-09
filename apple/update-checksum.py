#!/usr/bin/env python3
"""Rewrite the FSearchKit remote binary target after an ios-v* release."""

import re
import sys
from pathlib import Path

path = Path(__file__).resolve().parents[1] / "apple" / "FSearchKit" / "Package.swift"
url, checksum = sys.argv[1], sys.argv[2]
if not re.fullmatch(r"[0-9a-f]{64}", checksum):
    sys.exit(f"checksum must be 64 hex chars, got {checksum!r}")

text = path.read_text()
text, n_url = re.subn(r'let remoteURL = ".*"', f'let remoteURL = "{url}"', text, count=1)
text, n_sum = re.subn(
    r'let remoteChecksum = "[0-9a-fA-F]+"',
    f'let remoteChecksum = "{checksum}"',
    text,
    count=1,
)
if n_url != 1 or n_sum != 1:
    sys.exit(f"expected one URL and one checksum, replaced {n_url} and {n_sum}")
path.write_text(text)
