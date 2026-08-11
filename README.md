<p align="center">
  <img src="data/icons/hicolor/scalable/apps/io.github.superuser_miguel.Septima.svg" alt="Septima icon" width="96" height="96">
</p>

<h1 align="center">Septima</h1>

<p align="center"><strong>The archive tool that actually speaks modern-codec 7z — with a real compression-tuning UI.</strong></p>

Septima is a GTK4 / libadwaita front-end for [7-Zip ZS](https://github.com/mcmilk/7-Zip-zstd)
(`7zz`) on Linux. It is a GNOME-native app built specifically around modern
compression codecs — **Zstandard, Brotli, Fast-LZMA2** — with the kind of
codec-tuning controls no other Linux archive manager exposes.

It is an **archive tool, not a file manager**. It never links or vendors 7-Zip
code: a UI-free engine crate supervises the `7zz` binary as a subprocess.

> Status: **v0.4.0**, and past the awkward stage. Browse, extract, create with
> full tuning, batch operations and in-place editing all work today in a
> sandboxed Flatpak, with 1:1 coverage of what the bundled `7zz` can create on
> Linux. See the [roadmap](#roadmap).

<p align="center">
  <img src="docs/screenshots/codec-menu.png" alt="Septima's Add-to-Archive dialog with the modern-codec method menu open — LZMA2, Zstandard, Brotli, Fast-LZMA2, LZ4, LZ5, Lizard and more" width="640">
</p>

---

## Why Septima?

Modern-codec 7z with a tuning UI, in a GNOME-native app, is a gap nothing else fills:

| | Modern codecs (zstd/brotli/flzma2) | Real compression tuning | GNOME-native GUI | One-gesture `.tar.zst` |
|---|:---:|:---:|:---:|:---:|
| **File Roller / Ark** | ✗ | ✗ | ✓ / KDE | ✗ |
| **PeaZip** | partial | ✓ | ✗ (Qt) | partial |
| **7-Zip CLI / `7zz`** | ✓ | ✓ (flags) | ✗ | ✗ (two-step) |
| **Septima** | ✓ | ✓ | ✓ | ✓ |

Where Septima aims to *win*, not just match:

- **A real Add-to-Archive dialog** — format × codec × level, with the level range
  reacting to the codec (zstd 1–22, brotli 0–11, …), dictionary size, solid mode,
  threads, and a **live memory estimate** so you can see the cost before you commit.
- **"Optimize for executables"** — one switch for the BCJ filter, instead of the
  `-m0=bcj` folklore the Windows tool makes you learn.
- **Transparent modern tarballs** — create, browse, and extract a real
  `.tar.zst` / `.tar.xz` in one gesture, both ways.

## Features

- Browse any archive `7zz` can read (7z, zip, tar, xz, gzip, bzip2, zstd, rar…)
  in a details view: Name / Size / Packed / Method / Modified / CRC.
- Open an archive from the file chooser, your file manager ("Open With"), or by
  **dropping it onto the window**.
- Extract with **live progress, cancel, and password** support.
- **Transparent nested browse + extract** — open a `.tar.zst` / `.tgz` /
  `.tar.br` and see the files inside, not the intermediate `.tar`.
- **Create / Add to Archive** with full tuning:
  - Formats: **7z, zip, tar** (+ tar → zstd / xz / gzip / bzip2 / **brotli /
    LZ4 / LZ5 / Lizard**), and **raw single-file streams** — write a bare
    `.zst`, `.br`, `.lz4`, `.lz5`, `.xz`, `.gz` or `.bz2`.
  - Codecs: LZMA2, LZMA, PPMd, **Zstandard, Brotli, Fast-LZMA2, LZ4, LZ5,
    Lizard**, BZip2, Deflate, Deflate64, XZ, Store.
  - Level, dictionary size, solid mode, CPU threads, live memory estimate.
  - **Architecture / filter picker** — Delta, ARM64, ARM/ARMT, PPC, SPARC,
    RISC-V and BCJ2, alongside x86 BCJ.
  - **Lizard family × level** — pick among the four method families
    (fastLZ4 / LIZv1, ± Huffman) instead of one flat 10–49 slider.
  - Encryption — AES-256 with optional encrypted file names (7z), and
    **AES-256 instead of the weak ZipCrypto default** for zip.
  - Split into volumes (`.001`, `.002`, …), free-text advanced parameters.
- **Batch extract** — select or drop several archives and each unpacks into its
  own folder, with one shared password for encrypted sets.
- **Batch compress** — stage several items and create a separate archive for
  each, saved next to it.
- **Edit an archive in place** — delete or rename entries, plus a one-click
  **Test Archive** with the same live progress as extract.
- **Post-extract actions** — "Show in Files", and an optional
  delete-the-archive-afterwards toggle that understands split volumes.
- **Checksum calculator** — 13 digests: CRC-32/64, MD5, SHA-1, SHA-256/384/512,
  SHA3-256/512, BLAKE3, BLAKE2sp, XXH32/64 — with copy and
  verify-against-a-checksum-file.
- **Generate a checksum file** — optionally write a `.sha256` next to a new
  archive, one line per volume part.
- **Named compression presets** — save your tuning as a reusable preset.
- **Staged input list** — Add Files / Add Folder across locations, with per-item
  remove, drag-and-drop, and a live count + total size of what's staged.
- Ships as a **Flatpak** with `7zz` bundled — **portals only, no host filesystem
  access** by design.

<table>
<tr>
<td width="50%"><img src="docs/screenshots/create-dialog.png" alt="Create dialog with reactive level range and a live memory estimate"><br><em>Reactive tuning — level ranges follow the codec, with a live memory estimate.</em></td>
<td width="50%"><img src="docs/screenshots/create-options.png" alt="Executable optimization, split volumes, advanced switches, and encryption"><br><em>Executable optimization, split volumes, advanced switches, and encryption.</em></td>
</tr>
</table>

## Power-user switches

The create dialog's **Advanced → parameters** field is a free-text escape hatch:
whatever you type is passed straight to `7zz a`. It's for the long tail of
7-Zip's `-m` (method) and `-s` (store) switches that don't each earn a control.
A few of the useful ones:

**Codec fine-tuning**

| Switch | What it does |
|---|---|
| `-mfb=273` | Fast bytes / word size (LZMA/LZMA2, 5–273) — the biggest ratio knob after level |
| `-mmf=bt4` | Match finder (`hc4` / `bt2` / `bt3` / `bt4`) — speed vs ratio |
| `-mo=32 -mmem=256m` | PPMd model order / memory |
| `-mlc= -mlp= -mpb=` | LZMA literal-context / position bits (rarely needed) |

**Filters / architecture** (beyond the "Optimize for executables" switch)

- `-m0=ARM64 -m1=lzma2` — arch-specific executable filter (also `ARM` / `ARMT` / `PPC` / `SPARC` / `IA64`).
- `-m0=Delta:4 -m1=lzma2` — fixed-stride data (audio, tables, bitmaps).

**Metadata / storage**

- `-snl` — store symlinks *as links* (otherwise 7-Zip follows them). Handy on Linux.
- `-mtc=on -mta=on` / `-mtm=off` — store creation/access times, or drop mtime.
- `-ms=e` / `-ms=100m` — solid *by extension* / solid *block size*.
- `-mem=xchacha20poly1305` — new encryption methods, where the bundled `7zz` supports them.

**Behavior:** `-w<dir>` (working dir) · `-slp` (large pages) · `-u…` (update rules).

> Switches here are appended after the dialog's own options, so they override
> them. Params are split on spaces, so avoid switches containing spaces for now.
> Full list: [7-Zip command-line switches](https://documentation.help/7-Zip/).

## Install

### Recommended — signed repo, automatic updates

Install from the project's own signed Flatpak repo, and new releases arrive
with `flatpak update`:

```sh
flatpak install --user https://superuser-miguel.github.io/septima-repo/septima.flatpakref
flatpak run io.github.superuser_miguel.Septima
```

> This subscribes you to the repo (like Flathub does), so `flatpak update` — or
> GNOME Software — pulls new versions automatically. Every release is signed
> with the project's GPG key.

### Alternative — one-off bundle

Prefer a single file with no remote? Download **`Septima.flatpak`** from the
[latest release](https://github.com/superuser-miguel/septima/releases/latest):

```sh
flatpak install --user ./Septima.flatpak
flatpak run io.github.superuser_miguel.Septima
```

> The bundle has **no update path** — to move to a newer version, download it
> and reinstall (or switch to the signed repo above, which updates itself).

## Build from source

Septima builds and runs entirely inside the GNOME Flatpak sandbox.

```sh
flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --user --install --force-clean build-dir \
    build-aux/io.github.superuser_miguel.Septima.Devel.json
flatpak run io.github.superuser_miguel.Septima.Devel
```

For host development (needs `gtk4-devel`, `libadwaita-devel`, `blueprint-compiler`,
Meson, and `7zz` on `PATH`):

```sh
meson setup builddir -Dprofile=development
meson compile -C builddir
```

## Roadmap

### Shipped (through v0.4.0)

- [x] **Browse & extract** — any archive `7zz` reads, with live progress, cancel
      and password support.
- [x] **Create with full tuning** — format × codec × level, dictionary, solid,
      threads, live memory estimate, BCJ, volumes, encryption, advanced switches.
- [x] **Transparent nested browse + extract** — open, browse and extract a
      `.tar.zst` / `.tgz` / `.tar.xz` in one gesture, both ways.
- [x] **Named compression presets** — save and reuse tuning profiles.
- [x] **Staged input list** — Add Files / Add Folder across locations, per-item
      remove, and drag-and-drop from the file manager.
- [x] **Drop-to-open** — drag an archive onto the window to open it.
- [x] **Hash calculator** — CRC-32, SHA-256/512, SHA3-256, BLAKE3, xxHash, with
      copy and verify-against-a-checksum.
- [x] **Honest progress on big jobs** — an indeterminate "Scanning…" state while
      `7zz` enumerates the input, so a large selection no longer sits at 0%
      looking frozen.
- [x] **Selection totals** — the staged file count and total size are measured in
      the background and shown under Files, with a nudge when a selection is big
      enough to take a while.
- [x] **Responsive cancel** — Cancel takes effect in well under a second even
      while `7zz` is silent, and the half-written archive is deleted rather than
      left behind looking complete.
- [x] **Self-hosted Flatpak repo** — a signed OSTree repo + `.flatpakref` so
      `flatpak update` pulls new releases directly, no re-download needed.
- [x] **Post-extract actions** — a "Show in Files" action and an optional
      "delete the archive afterwards" toggle, including split-volume archives.
- [x] **Generate a checksum file** — optionally write a `.sha256` file next to
      a newly created archive (one line per volume part, for split archives).
- [x] **In-archive edit** — delete / rename entries (multi-select for delete),
      plus a "Test Archive" action with the same live progress as extract.
- [x] **Batch extract** — select or drop several archives at once and each
      extracts into its own new folder next to itself, no per-archive prompts.
- [x] **Batch compress** — stage several files/folders, flip "Create a separate
      archive for each item," and each one is compressed into its own archive
      saved next to it, instead of combining them into one.

Full `7zz` CLI parity, completed in **v0.4.0** — genuine 1:1 coverage of what
the bundled `7zz` can create on Linux:

- [x] **Complete hash calculator** — CRC-64, MD5, SHA-1, SHA-384, SHA3-512 and
      BLAKE2sp added, and xxHash split into XXH32 / XXH64 (13 digests in all).
      _(v0.3.0)_
- [x] **Zip encryption method** — choose AES-256 for zip instead of the weak
      ZipCrypto default. _(v0.3.0)_
- [x] **More format × codec combinations** — **XZ** and **Deflate64** inside
      zip, and **Brotli / LZ4 / LZ5 / Lizard** as `tar` post-compressors.
      _(v0.3.0)_
- [x] **Architecture / filter picker** — a real control for the filter family:
      Delta, ARM64, ARM/ARMT, PPC, SPARC, RISC-V and BCJ2, alongside x86 BCJ.
- [x] **Lizard family × level picker** — surface the four method families
      (fastLZ4 / LIZv1, ± Huffman) instead of one flat 10–49 slider.
- [x] **Standalone single-stream creation** — write a raw `.zst` / `.br` /
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

### In design — the headline 0.5.x feature

- **Batch-encrypt with a portable manifest** — compress a pile of files/folders,
  each into its own archive with a **freshly generated password**, and write a
  **portable manifest** (a CSV that doubles as an integrity record). You store
  that manifest wherever *you* keep secrets — KeePassXC, Bitwarden, `pass`, a
  GPG-encrypted file, the GNOME keyring — and later hand it back to Septima to
  **batch-decrypt** the whole set. Septima never talks to a vault's API; the
  manifest is just a file, so every vault is compatible by default. Sandbox-clean
  and built on your own tools. (Engine foundation already landed.)

### Later

- [ ] **Drag-out to extract** — drag entries out of an open archive to a folder
      to extract them (needs a drag source with on-demand extraction; drag-*out*
      support under Wayland / portals is the open question).
- [ ] **Promote key Advanced switches to real controls** — symlink handling
      (`-snl`), word size / fast bytes (`-mfb`), update modes (`-u`).
- [ ] **Free-space check** — show available space at the extract destination
      before starting.
- [ ] **Custom visual styling and app icon.**
- [ ] **More encryption methods** — XChaCha20-Poly1305, AES+XChaCha20 and
      friends via `-mem`, *blocked until* the bundled 7-Zip ZS ships them
      ([mcmilk/7-Zip-zstd#505](https://github.com/mcmilk/7-Zip-zstd/pull/505)).
- [ ] **Nautilus (Files) integration** — right-click "Extract Here" / "Extract
      to…" / "Compress…", like File Roller's extension. *Not a pure app
      feature*: a Nautilus extension loads into Nautilus's own host process,
      which the Flatpak sandbox can't do from inside Septima — this would need
      a small separate host-side package (e.g. distro-packaged, talking to
      Septima over D-Bus or the CLI), not something the Flatpak alone can ship.

## Acknowledgements

- **[7-Zip ZS](https://github.com/mcmilk/7-Zip-zstd)** by Tino Reichardt — the
  `7zz` binary Septima bundles and drives, which extends **7-Zip** by Igor Pavlov
  with Zstandard, Brotli, LZ4/LZ5, Lizard and Fast-LZMA2. Septima is *not* a fork
  of it; it is bundled unmodified as a separate Flatpak module.
- Built with **[gtk4-rs](https://gtk-rs.org/)**, **libadwaita**,
  **[Blueprint](https://gnome.pages.gitlab.gnome.org/blueprint-compiler/)**, and
  **Meson** — following the conventions of Amberol, Fractal and friends.

## License

Septima is **GPL-3.0-or-later**. The bundled 7-Zip ZS remains its own
LGPL-2.1-or-later / BSD-licensed work, built as a separate module.
