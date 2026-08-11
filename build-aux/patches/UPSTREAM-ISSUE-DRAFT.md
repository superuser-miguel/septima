# Design issue for mcmilk/7-Zip-zstd

**Filed 2026-07-31 as [mcmilk/7-Zip-zstd#528](https://github.com/mcmilk/7-Zip-zstd/issues/528).**
Kept here as the source of record; edit upstream if this changes. Title:

> Design proposal: AES-256-GCM + Argon2id for .7z (standards-only, one method, test vectors included)

---

Hi Tino,

Before writing a PR I'd like to agree the design, since the last crypto
contribution (#505) ran into trouble on scope and review effort rather than on
the idea. This proposal is deliberately the boring subset. I have a working
implementation, and I'm happy to change any of it — especially the codec ID —
before it becomes a PR.

## Motivation

Stock `.7z` encryption today is AES-256-CBC with an iterated-SHA-256 KDF and no
authentication. Two consequences:

- The KDF isn't memory-hard, so GPU/ASIC password cracking is cheap relative to
  a modern KDF.
- There's no authentication: a modified archive is detected only by CRC32, and a
  wrong password surfaces as "CRC failed" rather than a clean rejection.

The cipher isn't the weak point, so this proposal doesn't add ciphers — it adds
a memory-hard KDF and an AEAD mode.

## Scope: one method, two primitives

Method **AES256GCM**, selected with `-mem=AES256GCM` on `.7z` creation:

- **KDF:** Argon2id, RFC 9106, version 0x13
- **AEAD:** AES-256-GCM, NIST SP 800-38D
- Pack stream is `ciphertext || 16-byte tag`; a tag mismatch is a clean failure
  (wrong password or tampering), not a CRC error.

Explicitly **not** in scope: additional ciphers, cascades, custom
constructions, or anything requiring new analysis. No changes to existing
methods; the default stays `7zAES`.

## Proposed codec ID

`06 F7 11 01` — the established `F7 11 xx` vendor range under the `06` crypto
type byte, leaving `06 F7 11 02+` for future methods. **This is the main thing
I'd like your call on**, since it's the part that's expensive to change later.
If you'd rather allocate from a different range, or coordinate with the
published #505 fork so we don't collide, I'll follow whatever you prefer.

## Coder properties

Little-endian integers; parameters live in the header so extraction adapts to
whatever the writer chose:

```
Byte  0      props format version (1)
Byte  1      KDF id (1 = Argon2id v1.3)
Bytes 2..5   Argon2 m_cost, KiB (UInt32)
Bytes 6..9   Argon2 t_cost / passes (UInt32)
Bytes 10..13 Argon2 lanes (UInt32)
Byte  14     salt size (writer: 16; reader accepts 8..32)
Byte  15     iv size   (writer: 12; reader accepts 1..16)
then salt, then iv
```

Writer defaults m=256 MiB, t=3, p=4 (~0.2 s on a 12-thread desktop). The reader
enforces ceilings (m ≤ 2 GiB, t ≤ 64, p ≤ 64) so a hostile header can't demand
absurd resources. The password input is the UTF-16LE password bytes, same
convention as `7zAES`.

## Implementation

Roughly 1,400 lines excluding the vendored Argon2, all reference
implementations rather than hand-written crypto:

- `C/AesGcm.{h,c}` — streaming GCM over the existing AES kernels. GHASH is
  BearSSL's `ghash_ctmul64` (MIT, constant-time), vendored with only type and
  endian-macro changes.
- `C/argon2/` — PHC `phc-winner-argon2` release `20190702` vendored **verbatim**
  (CC0/Apache-2.0), including BLAKE2b.
- `CPP/7zip/Crypto/7zGcm.*` — the coder, modelled closely on `7zAes.cpp`
  (including its key-cache structure, which matters more here since each
  derivation costs ~0.2 s).
- 7z wiring: `-mem=` parsing, method display, encrypted-flag detection.
- `DOC/Methods-Extern.md` documents the ID and props layout.

Four logical commits, no drive-by changes. **Linux CLI only** for now — I don't
have a Windows environment to test the GUI/SFX side, so I'd rather not submit
untested changes there; happy to coordinate if you want that covered before
merging.

## Tests (shipped, not promised)

- AES-GCM: 3,036 vectors cross-checked against OpenSSL — AES-128/192/256, IV
  lengths 8..128, AAD, ragged streaming chunk sizes, round-trip, tamper
  rejection.
- Argon2: upstream's own suite plus RFC 9106 §5 known-answer vectors
  (argon2d/i/id with secret and associated data), passing on both the portable
  and SIMD fill paths.
- Archive level: `roundtrip.sh` — create/list/extract, wrong-password and
  1-byte-tamper rejection, `-mhe=on`, and a check that the default stays
  `7zAES`.

Happy to add a `7-Zip-Benchmarking/runtests.sh` run if that's useful.

## Compatibility

Opt-in only; without `-mem=AES256GCM` nothing changes. Stock 7-Zip lists such an
archive and fails extraction gracefully with "Unsupported Method" (verified
against 26.02), no crash.

## Status and expectations

This is already shipping in [Septima](https://github.com/superuser-miguel/septima)
(a Linux GUI over 7-Zip ZS) as a patch series on the pinned ZS tarball, with the
GUI probing `7zz i` so it degrades correctly against an unpatched binary. So
there's no time pressure from my side — if this lands upstream the patches
disappear and the archives stay compatible because the IDs match.

Would you be open to this in principle, and do you have a preference on the
codec ID?
