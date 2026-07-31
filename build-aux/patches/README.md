# 7-Zip ZS patches — Septima encryption expansion

Applied to the pinned `v26.02-v1.5.7-R2` tarball by the Flatpak manifest, in
numeric order. They add one archive encryption method to the bundled `7zz`:

**AES-256-GCM + Argon2id**, method ID `06 F7 11 01`, selected with
`-mem=AES256GCM` on `.7z` creation.

| Patch | Contents |
|---|---|
| 0001 | `C/AesGcm.{h,c}` — streaming AES-GCM (NIST SP 800-38D) over the in-tree AES kernel. GHASH is BearSSL `ghash_ctmul64` (MIT, constant-time), vendored unmodified apart from 7-Zip types. Plus `C/Util/AesGcmTest/` (3036 vectors vs OpenSSL). |
| 0002 | `C/argon2/` — PHC `phc-winner-argon2` release `20190702` vendored **verbatim** (tarball sha256 `daf972a8…`, CC0/Apache-2.0), plus RFC 9106 §5 known-answer tests. |
| 0003 | `CPP/7zip/Crypto/7zGcm.*` coder + 7z format wiring (`-mem=`, method display, encrypted-flag detection, makefiles) and the ID documented in `DOC/Methods-Extern.md`. |
| 0004 | `roundtrip.sh` acceptance tests — run by the manifest right after the build, so a broken crypto build fails the Flatpak build rather than shipping. |

## Design rules these follow

Standard constructions only; reference implementations rather than
hand-written crypto; test vectors for every primitive, shipped and run in the
build; no drive-by changes. (The cautionary tale is 7-Zip ZS PR #505, which
stalled on reviewability and non-standard constructions.)

## Compatibility

- **Default is unchanged.** Without `-mem=AES256GCM`, `.7z` encryption stays
  legacy `7zAES` and archives open anywhere.
- **Septima detects, never assumes.** `capabilities::aes256gcm_available()`
  probes `7zz i` at runtime; the UI option appears only when the engine in use
  supports it, so Septima remains correct against a stock or distro `7zz`.
- **Stock 7-Zip on a GCM archive** lists it and fails extraction gracefully
  ("Unsupported Method"), no crash. The create dialog says so plainly.

## Upstream

These are also the intended upstream contribution to
[mcmilk/7-Zip-zstd](https://github.com/mcmilk/7-Zip-zstd) — a design issue
staking out the codec ID and props layout comes first; `DOC/Methods-Extern.md`
in patch 0003 carries the format spec.

## Regenerating

The series lives on the `septima-crypto` branch of a 7-Zip-zstd checkout:

```sh
git format-patch v26.02-v1.5.7-R2..septima-crypto -o build-aux/patches --no-signature -N
```

Rebasing onto a newer ZS tag is usually cheap — the additions are almost all
new files; only patch 0003 touches existing ones.
