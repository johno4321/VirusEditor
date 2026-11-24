# Access Virus Rack factory singles (test ground truth)

`Bank_A.syx` and `Bank_B.syx` each contain the 128 factory single programs of the
Access Virus Rack (OS 1.x factory soundset) as raw SysEx single dumps:

```
F0 00 20 33 01 10 10 bb ss <256 data bytes> cs F7      (267 bytes per dump)
bb = 01 (Bank A) / 02 (Bank B), ss = 00..7F
cs = (0x10 + 0x10 + bb + ss + sum(data)) & 0x7F
```

| File         | SHA-256                                                            |
|--------------|--------------------------------------------------------------------|
| `Bank_A.syx` | `3345e29241837762033c15971f0e208950f0dd9a221ada9df07ccd538e9a28f2` |
| `Bank_B.syx` | `f66e643b19d723435a9473660450d9eda6b5f9f66231552d1289731338f9255c` |

Regenerate with `python testdata/factory/recover.py` (standard library only, e.g.
`uv run --no-project python testdata/factory/recover.py`). It prints a per-patch
report and exits non-zero if any patch cannot be recovered with certainty.

## Provenance

All inputs are in `VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/` (unmodified):

* `OSR_FactoryPatches_Bank_A.mid`, `OSR_FactoryPatches_Bank_B.mid` – Standard MIDI
  Files with the original hardware dumps. They were damaged by an LF→CR text
  conversion: every `0x0A` byte became `0x0D` (including SMF lengths and the
  program number of A010, which reads as `0x0D`). The checksum byte itself can
  also be hit, so a matching checksum alone does not prove a dump is intact.
* `Virus_rack_factory_patches.lib` – an Emagic SoundDiver library of the same
  256 patches. It was not text-converted, but SoundDiver does not store some bytes.
* `multis.mid` – a different soundset (multis plus their singles); only used as a
  cross-check where a single happens to be identical.

## SoundDiver `.lib` format (as far as needed)

* IFF-like: `FORM <u32 BE len> SSLB`, followed by chunks `<tag> <u32 BE len> <body>`
  **without** pad bytes (`LHDR`, `WSEQ`, then one `LENT` per library entry).
* `LENT` body: 12-byte entry header, then TLV records `<tag> <len> <value>` ended by
  a `00` tag. `len` is one byte, or two bytes `(hi | 0x80) lo` = 14-bit length.
  * `01` name (trailing spaces trimmed), `02` category text (e.g. `Pad, Off`),
  * `03` 250 bytes = dump data `[0..240)` + data `[250..256)` + 4 SoundDiver-specific
    bytes (meaning unknown, not needed) – i.e. the 256-byte dump with the
    10-byte name (bytes 240..249) cut out,
  * `06` label `A000`..`B127` = bank letter + program number.
* The first six `LENT`s are text entries (title, copyright, `Bank A`, …) without a
  `03` record; the remaining 256 are the patches.
* Compared with intact `.mid` bytes, the library always matches except at seven
  bytes it does not store (always `0` or a different value there): A3 (Contr 3),
  A112, A117, B41, B85, B100, B124 (data bytes 3, 112, 117, 169, 213, 228, 252).

## Method

For every program, starting from the `.mid` dump (the authoritative hardware data):

1. Every byte that is not `0x0D` in the `.mid` is kept as is. Outside the seven
   unstored bytes it must equal the library byte – no conflicts were found.
2. A `0x0D` byte outside the unstored set takes the library value (`0x0A` or `0x0D`).
3. A `0x0D` byte at an unstored position is decided by the checksum (treating a
   `0x0D` checksum byte as possibly an original `0x0A`); an undamaged identical copy
   in `multis.mid` would take precedence. Every case had exactly one solution.
4. The dump is written with `dd = 0x10`, the true program number and a freshly
   computed checksum.

`recover.py` asserts/flags all of the above; the final result was verified
independently (all 256 checksums valid, every byte equal to the library outside
the unstored set, and equal to the `.mid` wherever the `.mid` byte is not `0x0D`).

## Result

* Bank A: 128/128 recovered with certainty.
* Bank B: 128/128 recovered with certainty.

Checksum-only decisions (unstored byte that read `0x0D` in the `.mid`):

| Patch           | Byte                | Decision |
|-----------------|---------------------|----------|
| A010 BASSEY  NH | 3 (A3 Contr 3)      | `0x0A`   |
| A033 SpacePatHS | 117 (A117)          | `0x0A`   |
| A036 House 3 AS | 117 (A117)          | `0x0A`   |
| A052 DeepHousOK | 3 (A3 Contr 3)      | `0x0D`   |
| A066 Chaos   OK | 213 (B85)           | `0x0A`   |
| A106 FM Bass JS | 3 (A3 Contr 3)      | `0x0D`   |
| B006 Compres2 K | 213 (B85)           | `0x0A`   |
| B010 Vi-bass RP | 3 (A3 Contr 3)      | `0x0A`   |
| B013 Polaris    | 3 (A3 Contr 3)      | `0x0D`   |
| B025 Tension... | 213 (B85)           | `0x0D`   |
| B029 GeneratoMS | 117 (A117)          | `0x0D`   |

Patches that could not be recovered with certainty: **none**.
