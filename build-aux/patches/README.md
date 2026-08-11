# 7-Zip ZS patches — Septima encryption expansion

Apply to the pinned `v26.02-v1.5.7-R2` tarball in numeric order. They add one
archive encryption method to the bundled `7zz`:

**AES-256-GCM + Argon2id**, method ID `06 F7 11 01`, selected with
`-mem=AES256GCM` on `.7z` creation.

| Patch | Contents |
|---|---|
| 0001 | `C/AesGcm.{h,c}` — streaming AES-GCM (NIST SP 800-38D) over the in-tree AES kernel. GHASH is BearSSL `ghash_ctmul64` (MIT, constant-time), vendored unmodified apart from 7-Zip types. Plus `C/Util/AesGcmTest/` (3036 vectors vs OpenSSL). |
| 0002 | `C/argon2/` — PHC `phc-winner-argon2` release `20190702` vendored **verbatim** (tarball sha256 `daf972a8…`, CC0/Apache-2.0), plus RFC 9106 §5 known-answer tests. |
| 0003 | `CPP/7zip/Crypto/7zGcm.*` coder + 7z format wiring (`-mem=`, method display, encrypted-flag detection, the header-encryption path, makefiles) and the ID documented in `DOC/Methods-Extern.md`. |
| 0004 | `roundtrip.sh` — 9 acceptance checks, run right after the build when the series is enabled, so a broken crypto build fails the Flatpak build rather than shipping. |

## ⛔ Currently gated OFF

The Flatpak manifest does **not** apply these patches yet — the shipped `7zz`
is pristine, and the runtime probe keeps the UI option hidden. The gate stays
closed until the codec-ID question is settled upstream
([mcmilk/7-Zip-zstd#528](https://github.com/mcmilk/7-Zip-zstd/issues/528)).
To flip it: restore the four `"type": "patch"` sources in
`build-aux/io.github.superuser_miguel.Septima.json` and the
`roundtrip.sh` build command between `make` and `install` (git history of that
file has the exact hunk).

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
- **Encrypted headers (`-mhe=on`) use the same cipher as the data.** This needs
  saying because it did not always hold: the header-only encode path in
  `7zOut.cpp` builds its own `CCompressionMethodMode`, and until it carried
  `CryptoMethod` across it silently kept the `k_AES` default. Since
  `7zHandlerOut.cpp` clears `CompressMainHeader` when `numItems < 2`, a
  **one-item** archive written with `-mem=AES256GCM -mhe=on` protected its file
  names with legacy AES-256-CBC + the SHA-256 KDF — the exact KDF this series
  exists to replace — and any stock `7zz` could list them with the password.
  Fixed in 0003; check 8 in `roundtrip.sh` is the regression test and fails
  against a build without the fix.

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

The `From:` lines land in this repo, so the branch commits must be authored as
`superuser-miguel <16271056+superuser-miguel@users.noreply.github.com>` — a
private address in a patch header is published the moment these files are.

Rebasing onto a newer ZS tag is usually cheap — the additions are almost all
new files; only patch 0003 touches existing ones.

Verify a regenerated series end-to-end the way the manifest will use it —
pristine tarball, `patch -p1`, build, acceptance script:

```sh
git archive v26.02-v1.5.7-R2 | tar -x -C /tmp/pristine
cd /tmp/pristine && for f in build-aux/patches/000*.patch; do patch -p1 < "$f"; done
make -j -C CPP/7zip/Bundles/Alone2 -f makefile.gcc
sh C/Util/AesGcmTest/roundtrip.sh CPP/7zip/Bundles/Alone2/_o/7zz   # expect 9/9
```
