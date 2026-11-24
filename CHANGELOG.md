# Changelog

What changed in each release of Septima, newest first. The same notes, in
shorter form, are in the app's About window and on the
[releases page](https://github.com/superuser-miguel/septima/releases).

## 0.7.0 — 2026-11-24

Big batches, and more ciphers.

- **A cap on how many archives run at once** — a batch used to start one
      `7zz` per archive, all together, so a hundred archives meant a hundred
      processes. Now half as many as the CPU has threads run (at least 2, at
      most 8) and the rest wait. Creating archives runs fewer side by side
      when each one already uses every thread or a large share of free memory.
- **A job list that copes** — a running/waiting count, *Cancel All*,
      scrolling after five rows, and waiting archives that can be cancelled
      before they start. A long list of failures scrolls inside its dialog.
- **Batch extract has its own way in** — *Extract Several Archives* in the
      menu, on the start page and on Ctrl+Shift+E; opening several archives
      from Files offers it too.
- **Four more encryption methods for 7z** — XChaCha20-Poly1305 and the
      three AES/XChaCha20 cascades from
      [mcmilk/7-Zip-zstd#505](https://github.com/mcmilk/7-Zip-zstd/pull/505),
      each offered only when the `7zz` in use has it, and labelled with where
      it opens (7-Zip ZS 26.05 or newer, not stock 7-Zip). None is ranked
      above another, and AES-256 stays the default.
- **Presets don't downgrade quietly** — a preset whose encryption method
      isn't available says so instead of switching to AES-256 without a word.
- **Passwords file: a clearer folder prompt** — the sandbox lets Septima
      read the passwords file you open but not its neighbours, so it asks for
      the folder; the prompt now says why and opens in the right place.

## 0.6.0 — 2026-11-20

Keyboard, help, and no silent overwrites.

- **Extracting never replaces a file without asking** — before 0.6, an
      extract into a folder that already held a file of the same name replaced
      it silently. Septima now checks first and, only on a clash, asks: *Keep
      both* (default), *Skip* or *Replace*. Batch extract asks when an
      archive's folder already exists. *Skip* can't be combined with deleting
      the archive afterwards, since the skipped files would go with it.
- **Keyboard shortcuts** throughout, listed on Ctrl+?. Delete and F2 act on
      the file list only, so they still work inside text fields.
- **What Septima Can Do** (F1), generated from the code and kept honest by
      a test.
- **About window** with release notes and the bundled 7-Zip ZS build.

## 0.5.2 — 2026-11-19

- Bundles 7-Zip ZS 26.05 (v1.5.7, Release 1), which fixes a buffer overflow in
  the brotli decoder (CVE-2026-111497) and a use-after-free when extracting
  standalone `.zst`, `.br`, `.lz4`, `.lz5` and `.liz` files. Truncated or
  corrupt archives now report an error instead of extracting silently.

## 0.5.1 — 2026-09-28

Batches that finish quietly.

- **One notice per batch, not one per archive** — a batch of 50 used to
      queue 50 notices (100 with checksum files on), still scrolling past long
      after the work was done. Now a batch ends with one summary — *"Extracted
      48 archives · 2 failed"* — with a Show in Files button.
- **Failures collected, not interrupting** — one dialog at the end lists
      what failed and why, instead of an error pop-up per archive mid-run.
- **One password question for a batch extract** — encrypted archives in a
      batch are held until the rest finish, then Septima asks once for all of
      them (a wrong password asks again; Cancel skips them).
- **No password box by default** — the batch-extract dialog hides the
      password field behind a "These archives are password-protected" switch,
      so a batch of ordinary archives no longer looks like it wants one.
- **Same-name items no longer collide** — `photos.zip` + `photos.7z` in one
      batch extract to `photos/` and `photos_2/` instead of overwriting each
      other; `report.txt` + `report.pdf` compress to `report.7z` and
      `report_2.7z` instead of two jobs writing one file.

## 0.5.0 — 2026-08-16

Batch encryption with a portable passwords file.

The headline feature: encrypt *a pile of things* without inventing a password
per item or reusing one across all of them.

- **Unique password per archive** — in batch mode ("create a separate
      archive for each item"), every archive gets its own 64-character password
      drawn from the kernel CSPRNG, and the set is recorded in a **passwords
      file** you save where you like.
- **The passwords file is just a file** — readable JSON with one entry per
      archive: name, what it was made from, the password, its SHA-256, when it
      was written, and the cipher in plain words ("7z, AES-256, encrypted
      headers") — the thing nobody remembers six months later. Keep it in
      KeePassXC, Bitwarden, `pass`, a vault, or a drawer. Septima never talks to
      a password manager's API, so every one of them works by default.
- **Optional password protection for the file itself** — standard symmetric
      OpenPGP via the runtime's `gpg -c`: no keys, no keyring, no host access,
      and any `gpg` anywhere opens it. Chosen **before** the run, so a protected
      run never leaves a plaintext moment on disk; declining shows a plain-words
      warning instead of silently writing secrets next to the archives.
- **Crash-safe ordering** — all passwords are written atomically *before*
      the first archive is created, and the file is rewritten (temp + rename)
      after each archive completes. No archive can exist whose password doesn't.
- **The other half: batch decrypt** — open a passwords file with the Open
      button, by dropping it in, or from the command line, and Septima offers to
      extract every archive it lists with its recorded password. A per-entry
      picker with select-all/clear (and a filter past ten rows) means you can
      take just the two you need; missing archives and empty passwords are
      skipped loudly, and only basenames are trusted, so a hand-edited
      `../evil` can't walk out of the folder.
- **Drag-out to extract** — the missing half of drag and drop: select
      entries in an open archive, drag them into Files, and they arrive
      extracted. Directories bring their subtree.
- Presets now capture the encryption method, the checksum-file switch and
      the batch password choices, instead of dropping them.

## 0.1.0 to 0.4.0 — July and August 2026

Everything the first four releases added, as one list.

- **Browse & extract** — any archive `7zz` reads, with live progress, cancel
      and password support.
- **Create with full tuning** — format × codec × level, dictionary, solid,
      threads, live memory estimate, BCJ, volumes, encryption, advanced switches.
- **Transparent nested browse + extract** — open, browse and extract a
      `.tar.zst` / `.tgz` / `.tar.xz` in one gesture, both ways.
- **Named compression presets** — save and reuse tuning profiles.
- **Staged input list** — Add Files / Add Folder across locations, per-item
      remove, and drag-and-drop from the file manager.
- **Drop-to-open** — drag an archive onto the window to open it.
- **Hash calculator** — CRC-32, SHA-256/512, SHA3-256, BLAKE3, xxHash, with
      copy and verify-against-a-checksum.
- **Honest progress on big jobs** — an indeterminate "Scanning…" state while
      `7zz` enumerates the input, so a large selection no longer sits at 0%
      looking frozen.
- **Selection totals** — the staged file count and total size are measured in
      the background and shown under Files, with a nudge when a selection is big
      enough to take a while.
- **Responsive cancel** — Cancel takes effect in well under a second even
      while `7zz` is silent, and the half-written archive is deleted rather than
      left behind looking complete.
- **Self-hosted Flatpak repo** — a signed OSTree repo + `.flatpakref` so
      `flatpak update` pulls new releases directly, no re-download needed.
- **Post-extract actions** — a "Show in Files" action and an optional
      "delete the archive afterwards" toggle, including split-volume archives.
- **Generate a checksum file** — optionally write a `.sha256` file next to
      a newly created archive (one line per volume part, for split archives).
- **In-archive edit** — delete / rename entries (multi-select for delete),
      plus a "Test Archive" action with the same live progress as extract.
- **Batch extract** — select or drop several archives at once and each
      extracts into its own new folder next to itself, no per-archive prompts.
- **Batch compress** — stage several files/folders, flip "Create a separate
      archive for each item," and each one is compressed into its own archive
      saved next to it, instead of combining them into one.

Full `7zz` CLI parity, completed in **v0.4.0** — genuine 1:1 coverage of what
the bundled `7zz` can create on Linux:

- **Complete hash calculator** — CRC-64, MD5, SHA-1, SHA-384, SHA3-512 and
      BLAKE2sp added, and xxHash split into XXH32 / XXH64 (13 digests in all).
      _(v0.3.0)_
- **Zip encryption method** — choose AES-256 for zip instead of the weak
      ZipCrypto default. _(v0.3.0)_
- **More format × codec combinations** — **XZ** and **Deflate64** inside
      zip, and **Brotli / LZ4 / LZ5 / Lizard** as `tar` post-compressors.
      _(v0.3.0)_
- **Architecture / filter picker** — a real control for the filter family:
      Delta, ARM64, ARM/ARMT, PPC, SPARC, RISC-V and BCJ2, alongside x86 BCJ.
- **Lizard family × level picker** — surface the four method families
      (fastLZ4 / LIZv1, ± Huffman) instead of one flat 10–49 slider.
- **Standalone single-stream creation** — write a raw `.zst` / `.br` /
      `.lz4` / `.lz5` / `.xz` / `.gz` / `.bz2` stream, not only `tar` + a
      compressor.

**v0.4.0 also repairs Brotli creation.** Every `.br` and `.tar.br` written by
v0.3.0 was corrupt — it failed its own integrity test — because Septima always
passed `-mmt`, and that flag silently switches `7zz` between two incompatible
brotli stream formats. Fixed on the write *and* read side; existing broken files
can't be repaired, so recreate them. Also reported upstream
([#352](https://github.com/mcmilk/7-Zip-zstd/pull/352#issuecomment-5144178288),
and the detection design in
[#538](https://github.com/mcmilk/7-Zip-zstd/issues/538)) — see
[the write-up](https://superuser-miguel.github.io/septima/blog/2026-07-27-brotli-mmt-corruption.html).
`.tar.br` / `.tar.lz5` / `.tar.liz` are transparently browsable again too.
