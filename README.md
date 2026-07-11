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

> Status: early but working. Browse, extract, and create-with-tuning all
> function today in a sandboxed Flatpak. See the [roadmap](#roadmap).

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
- **Transparent modern tarballs** — create a real `.tar.zst` / `.tar.xz` in one
  gesture (transparent *browsing* of them is on the roadmap).

## Features

- Browse any archive `7zz` can read (7z, zip, tar, xz, gzip, bzip2, zstd, rar…)
  in a details view: Name / Size / Packed / Method / Modified / CRC.
- Extract with **live progress, cancel, and password** support.
- **Create / Add to Archive** with full tuning:
  - Formats: **7z, zip, tar** (+ tar → zstd/xz/gzip/bzip2).
  - Codecs: LZMA2, LZMA, PPMd, **Zstandard, Brotli, Fast-LZMA2, LZ4, LZ5, Lizard**,
    BZip2, Deflate, Store.
  - Level, dictionary size, solid mode, CPU threads, live memory estimate.
  - Executable-optimization (BCJ), free-text advanced parameters.
  - Encryption (AES-256) with optional encrypted file names (7z).
  - Split into volumes (`.001`, `.002`, …).
- Ships as a **Flatpak** with `7zz` bundled — **portals only, no host filesystem
  access** by design.

<table>
<tr>
<td width="50%"><img src="docs/screenshots/create-dialog.png" alt="Create dialog with reactive level range and a live memory estimate"><br><em>Reactive tuning — level ranges follow the codec, with a live memory estimate.</em></td>
<td width="50%"><img src="docs/screenshots/create-options.png" alt="Executable optimization, split volumes, advanced switches, and encryption"><br><em>Executable optimization, split volumes, advanced switches, and encryption.</em></td>
</tr>
</table>

## Install / Build

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

- [ ] **Transparent nested browsing** — open a `.tar.zst` and see the files
      inside (not just the outer tar). Completes the "one-gesture both ways" goal.
- [ ] **Named compression presets** — save and reuse tuning profiles.
- [ ] In-archive delete / rename; hash calculator (BLAKE3, SHA-3, xxHash).
- [ ] Lizard family × level picker.
- [ ] Custom visual styling and app icon.
- [ ] Flathub submission.

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
