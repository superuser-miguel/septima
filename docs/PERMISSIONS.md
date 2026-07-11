# Permissions

Septima requests the **minimum** to be a windowed GTK app, and reaches your
files **only through XDG Desktop Portals** — it has **no direct filesystem
access** (no `--filesystem=host`, `--filesystem=home`, or any path). Opening,
extracting to, and creating archives all go through the file-chooser portals, so
you grant access to exactly the files and folders you pick, nothing more.

## `finish-args`

| Permission | Why it exists | Without it |
|---|---|---|
| `--socket=wayland` | Draw the window on Wayland | No window on Wayland sessions |
| `--socket=fallback-x11` | Draw the window on X11 (only when Wayland is absent) | No window on X11 sessions |
| `--share=ipc` | Shared-memory with the display server (required by X11 / for performant rendering) | Rendering glitches or failure to start under X11 |
| `--device=dri` | GPU access for hardware-accelerated GTK rendering | Slow (software) rendering |

## Not requested (by design)

- **No filesystem access.** File access is entirely portal-mediated
  (`FileChooser`, folder selection, save). This is the core of the design.
- **No network.** Septima does not talk to the network at runtime.
- **No `--talk-name` / D-Bus** access beyond the portals every app gets.

## Bundled binary

`7zz` (7-Zip ZS) is bundled inside the sandbox at `/app/bin/7zz` and runs as a
child process. It inherits **only** the sandbox's permissions — it cannot reach
anything Septima itself cannot.

> Maintainer note: keep this file in sync with the manifest. A change to
> `finish-args` without a matching change here is an incomplete change.
