#!/usr/bin/env python3
"""Recover the Access Virus Rack factory single banks A and B.

Inputs (read-only) from VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/:
  * OSR_FactoryPatches_Bank_A.mid / _Bank_B.mid - SMF files holding the original
    hardware SysEx dumps, damaged by an LF->CR conversion (every 0x0A is 0x0D).
  * Virus_rack_factory_patches.lib - Emagic SoundDiver library holding the same
    256 patches undamaged, but with a few parameters not stored (zeroed).
  * multis.mid - used only as an additional cross-check where a patch matches.

Outputs: Bank_A.syx, Bank_B.syx (128 single dumps each) next to this script.
Prints a per-patch report; exits non-zero if any patch is not recovered with
certainty. Standard library only: `python testdata/factory/recover.py`.
"""
import itertools
import re
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SRC = HERE.parent.parent / "VirusOSR200_INT" / "OSR200_INT" / "Goodies" / "Factory soundset"

DEVICE_ID = 0x10
CR, LF = 0x0D, 0x0A
NAME = slice(240, 250)
# Single-dump bytes that the SoundDiver library does not store (always 0 there):
# A3 Contr 3, A112, A117, B41, B85, B100, B124 (Category2).
LIB_BLIND = {3, 112, 117, 128 + 41, 128 + 85, 128 + 100, 128 + 124}

DUMP_RE = re.compile(rb"\x00\x20\x33\x01(.)\x10(.)(.)", re.S)


def checksum(dd, bb, ss, data):
    return (dd + 0x10 + bb + ss + sum(data)) & 0x7F


def mid_dumps(path):
    """Single dumps in file order: (dd, bb, ss, data[256], cs)."""
    raw = path.read_bytes()
    out = []
    for m in DUMP_RE.finditer(raw):
        s = m.start()
        if raw[s + 265] != 0xF7:
            raise ValueError(f"{path.name}: no F7 after dump at {s}")
        out.append((m[1][0], m[2][0], m[3][0], raw[s + 8:s + 264], raw[s + 264]))
    return out


def lib_patches(path):
    """Parse the SoundDiver library into {(bank, prog): (name, 256-byte data)}.

    Layout: 'FORM' <u32 BE len> 'SSLB', then chunks <tag> <u32 BE len> <body>
    with no pad bytes. Each 'LENT' body = 12-byte entry header followed by
    TLV records <tag> <len> <value> terminated by tag 0x00, where len is one
    byte, or two bytes (hi | 0x80, lo) as a 14-bit value. Records seen:
    1 = name (trailing spaces trimmed), 2 = category text,
    3 = 250 bytes: dump data[0:240] + data[250:256] + 4 SoundDiver bytes,
    6 = label such as 'A000' / 'B127' (bank letter + program).
    """
    raw = path.read_bytes()
    assert raw[:4] == b"FORM" and raw[8:12] == b"SSLB"
    off, end, out = 12, 8 + struct.unpack(">I", raw[4:8])[0], {}
    while off < end:
        tag, ln = raw[off:off + 4], struct.unpack(">I", raw[off + 4:off + 8])[0]
        body = raw[off + 8:off + 8 + ln]
        off += 8 + ln
        if tag != b"LENT":
            continue
        p, recs = 12, {}
        while p < len(body) and body[p] != 0:
            t, n = body[p], body[p + 1]
            p += 2
            if n & 0x80:
                n = ((n & 0x7F) << 7) | body[p]
                p += 1
            recs[t] = body[p:p + n]
            p += n
        if 3 not in recs:
            continue
        label, blob = recs[6].decode("ascii"), recs[3]
        assert len(blob) == 250 and label[0] in "AB", label
        name = recs[1].ljust(10, b" ")
        out[("AB".index(label[0]) + 1, int(label[1:]))] = (name, blob[:240] + name + blob[240:246])
    return out


def recover(bank, prog, dump, lib_data, extra_copies):
    """Return (data, status, note). status: 'ok' or 'uncertain'."""
    dd, bb, ss, mid, cs = dump
    expected_ss = {prog, CR if prog == LF else prog}
    assert bb == bank and ss in expected_ss, (bank, prog, bb, ss)
    out = bytearray(mid)
    blind, conflicts, fixed = [], [], 0
    for i in range(256):
        if i in LIB_BLIND:
            if mid[i] == CR:
                blind.append(i)
        elif mid[i] != lib_data[i]:
            if mid[i] == CR and lib_data[i] == LF:
                out[i] = LF
                fixed += 1
            else:
                conflicts.append(i)
    # Undamaged copies of this patch elsewhere (multis.mid) decide blind bytes.
    for copy in extra_copies:
        for i in list(blind):
            if copy[i] != CR:
                out[i] = copy[i]
                blind.remove(i)
    targets = {cs} | ({LF} if cs == CR else set())
    solutions = []
    for k in range(len(blind) + 1):
        for flips in itertools.combinations(blind, k):
            cand = bytearray(out)
            for i in flips:
                cand[i] = LF
            if checksum(DEVICE_ID, bank, prog, cand) in targets:
                solutions.append((flips, bytes(cand)))
    note = f"{fixed} byte(s) 0D->0A from lib"
    if conflicts:
        return bytes(out), "uncertain", note + f"; mid/lib conflict at {conflicts}"
    if len(solutions) == 1:
        flips = solutions[0][0]
        if flips:
            note += f"; checksum resolves byte(s) {list(flips)} to 0A"
        if blind and len(flips) < len(blind):
            note += f"; checksum keeps byte(s) {[i for i in blind if i not in flips]} as 0D"
        return solutions[0][1], "ok", note
    if not solutions:
        return bytes(out), "uncertain", note + "; no 0D/0A assignment matches the checksum"
    opts = " or ".join(str(list(f)) for f, _ in solutions)
    return solutions[0][1], "uncertain", note + f"; blind bytes {blind}: flips {opts} all fit checksum"


def main():
    lib = lib_patches(SRC / "Virus_rack_factory_patches.lib")
    multis = mid_dumps(SRC / "multis.mid")
    uncertain = []
    for bank, letter in ((1, "A"), (2, "B")):
        dumps = mid_dumps(SRC / f"OSR_FactoryPatches_Bank_{letter}.mid")
        assert len(dumps) == 128
        syx = bytearray()
        for prog, dump in enumerate(dumps):
            name, lib_data = lib[(bank, prog)]
            # Use multis.mid copies whose bytes agree everywhere except CR / blind spots.
            copies = [
                m[3] for m in multis
                if all(a == b or CR in (a, b) or i in LIB_BLIND
                       for i, (a, b) in enumerate(zip(m[3], lib_data)))
            ]
            data, status, note = recover(bank, prog, dump, lib_data, copies)
            label = f"{letter}{prog:03d} {name.decode('latin-1')}"
            if copies:
                note += f"; {len(copies)} multis.mid copy(ies)"
            print(f"{label}  {status:9s} {note}")
            if status != "ok":
                uncertain.append((label, note))
            syx += bytes([0xF0, 0x00, 0x20, 0x33, 0x01, DEVICE_ID, 0x10, bank, prog])
            syx += data + bytes([checksum(DEVICE_ID, bank, prog, data), 0xF7])
        (HERE / f"Bank_{letter}.syx").write_bytes(syx)
    print(f"\n{256 - len(uncertain)}/256 recovered with certainty")
    for label, note in uncertain:
        print(f"UNCERTAIN {label}: {note}")
    return 1 if uncertain else 0


if __name__ == "__main__":
    sys.exit(main())
