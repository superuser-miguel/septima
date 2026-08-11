# Septima — Concepts

_Design sketches and feature ideas, pre-decision. Not a roadmap; nothing here is
committed to being built. Last updated 2026-07-25._

---

## Batch encrypt with generated passwords + a manifest

### Origin — the real 2022-2024 shell workflow (`~/my-progs/7z_files/`)

This feature is the direct descendant of hand-rolled scripts (`007za_all_folders*.sh`,
`Double_007za_all_folders.sh`, `*Decryptor*.sh`). What they actually did:

1. Sanitize folder names (`prename`: `,`/space → `_`, collapse dupes).
2. Per folder: random numeric prefix + **random 64-char alphanumeric password**
   (`tr -dc '[:alnum:]' </dev/urandom`), a running sequence number, an archive
   name `${prefix}_archive_${seq}`.
3. Create each: `7za a -t7z -m0=lzma -mx=9 -mfb=64 -md=32m -ms=off -mhe=on -p<pw>`
   — LZMA max, **encrypted headers (-mhe)**, **solid off**. (Septima can express
   all of this today: filter/method/level/dict + encrypt-file-names + no-solid.)
4. Append `dir,prefix,seq,archive,password` to a **plaintext CSV** in `/home`.
5. A separate decryptor **reads that CSV** and batch-extracts each with its logged
   password, `rm`-ing the archive after.

**Three insights this locks in:**

- **The manifest is a CSV with columns** (`dir,seq,archive,password`) — which is
  *natively* a password-manager import format (KeePassXC/Bitwarden). So "export
  to your vault" isn't a separate backend; it's the manifest shaped right.
- **There is a READ side, not just write.** The loop is
  **batch-encrypt → manifest → batch-decrypt-from-manifest**. The manifest is the
  bridge, and it's exactly the `{source: destination}` map (source→dest+password).
- **The entire point is to fix step 4** — the plaintext CSV in `/home` (the
  "improperly kept" file). Encrypted-file / vault / keyring (user's choice) is the
  fix for that one mistake.

- **Genesis of the vault angle:** a commenter on a 7-Zip ZS issue floated a
  password-manager add-on — combined with the workflow above, that's what seeded
  the pluggable-backends direction.

### The workflow (user's, verbatim intent)

> 7z a lot of directories, password-protect each one, generate a 64-digit
> alphanumeric password per archive, and dump the passwords into a log file to
> open later.

Today this is a hand-rolled shell loop. Septima already does most of it:
**batch compress** (one archive per staged item, sibling folders) shipped, and
per-archive encryption exists. The two missing pieces are **(a) generate a
unique strong password per archive** and **(b) record archive→password
somewhere retrievable**.

### Why it fits

- Builds directly on batch compress — same "one archive per item" engine path,
  just with a distinct password per item instead of one shared (or none).
- Septima already writes sidecar files (`.sha256` checksums), so writing a
  manifest is a known pattern.
- Related to the parked `{source: destination}` manifest idea — both point at a
  **manifest-driven batch** mode.

### ⚠️ Security is the whole design problem

Generating passwords is trivial and safe (OS CSPRNG). **Storing them is the
hard part, and a plaintext password log is a genuine footgun**, so the concept
has to lead with this, not bolt it on.

- **The log is a master key.** Anyone who has the archives *and* the log has
  everything. A `passwords.txt` sitting next to the `.7z` files defeats the
  encryption entirely. Whatever we build must make the safe path the easy path.
- **Entropy is not the concern.** 64 alphanumeric chars ≈ 380 bits — absurdly
  strong. The weak link is always storage/handling, never the password itself.
- **Sandbox reality.** Septima is Flatpak, portals-only. Writing a manifest
  goes through the file portal (user picks the location). Reading/writing the OS
  keyring would need the Secret portal.

### Manifest options, worst → best for security

1. **Plaintext `.txt` / `.csv`** — what the user does now. Convenient, portable,
   and the master-key risk above. If offered at all, it should be an **explicit
   opt-in with a blunt warning**, and Septima should *nudge* the file somewhere
   other than next to the archives.
2. **Encrypted manifest** — write the archive→password list into a single
   password-protected file (a `.7z` we encrypt with one master passphrase, or
   an age/GPG file). Reduces N secrets to **one** you have to protect. Strong
   default: the user memorises/vaults one passphrase, not 200.
3. **Password-manager export** — emit a CSV in a shape KeePassXC / Bitwarden /
   `pass` can import (title = archive path, password = generated). Passwords
   land in a real vault, never a flat file. Great for users who already run one.
4. **OS keyring (libsecret via the Secret portal)** — store each archive's
   password keyed by its path. Elegant and sandbox-native; also opens the door
   to Septima **auto-unlocking on re-open** ("Septima remembers this archive's
   password"). Downside: not portable across machines, and it's the most work.

**Provisional stance:** default to **#2 (encrypted manifest)** or **#3
(password-manager CSV)**; make **#1 (plaintext)** a deliberate, warned choice;
treat **#4 (keyring)** as a later, higher-effort enhancement that unlocks the
auto-unlock feature too.

### Integrating with the user's crypto stack (gpg / KeePassXC / Seahorse)

The user already runs **KeePassXC, gpg, Seahorse (gnome-keyring), and ssh**, and
suggested either "a GPG sign/encrypt action" or "a flatpak-spawn to KeePassXC."
Both are real, but they cost Septima's core promise —
*"portals only, no host filesystem access"* — by very different amounts. Ranked
by how well they keep the sandbox intact:

**B1. Bundled OpenPGP, encrypt to the user's *public* key (sandbox stays clean).**
Ship a Rust OpenPGP impl (`sequoia-openpgp`) or a bundled `gpg`; the user selects
/exports their public key once; Septima encrypts the manifest to it and writes a
`.gpg`/`.asc` via the file portal. They decrypt later with their normal
gpg/Seahorse. **Only ever touches the public key — never the secret key, never
`~/.gnupg`, no host-spawn.** Preserves the sandbox completely and still yields a
portable encrypted file. Cost: a heavier dep + a one-time "pick your key" step,
and GPG-interop fiddliness (recipient/key formats).

**B2. Host gpg via `flatpak-spawn --host gpg` (pragmatic, but punches a hole).**
Least code, and it reuses the user's *real* gpg — their keyring, their pinentry,
their exact setup — to encrypt (or sign) the manifest. But it needs
`--talk-name=org.freedesktop.Flatpak`, i.e. the app can run **arbitrary host
commands**. For a security-adjacent tool that advertises a tight sandbox, that's
an ironic and real tradeoff — it's the broadest permission of any option here.

**C. Secret Service / libsecret — and this is the *good* KeePassXC path.**
Store each archive's password in the Secret Service via libsecret. The elegant
part: **KeePassXC can itself *be* the Secret Service provider** (Settings →
Secret Service Integration), and so can gnome-keyring (Seahorse). So "store via
the Secret Service" transparently lands in **whichever the user runs** — no
KeePassXC-specific code, no spawning. From the sandbox this needs
`--talk-name=org.freedesktop.secrets` (a narrow, keyring-only D-Bus permission —
far tighter than host-spawn). Bonus: this is also what enables **Septima
auto-unlocking an archive on re-open**. Cost: per-machine, not a portable file.

**D. Direct `keepassxc-cli` spawn (not recommended).**
`flatpak-spawn --host keepassxc-cli add …` can append an entry to a `.kdbx`, but
needs the DB path *and* its unlock password passed in — clunky, and it still
carries the host-spawn hole from B2. The Secret-Service route (C) is the strictly
better way to reach KeePassXC.

**Reading of the options:** for the **portable "encrypted log I open later"** the
user described, **B1** is the most honest match — it keeps the sandbox promise,
touches only public-key material, and produces a file their existing gpg/Seahorse
decrypts. **C** is the best "just remember it / auto-unlock" path and the right
way to reach KeePassXC, worth doing on its own merits. **B2/D (host-spawn)** are
the shortcuts, and the doc should treat weakening the sandbox as a cost the user
opts into knowingly — not a default.

### ⛔ DESIGN DECISION DEFERRED — it's NOT "pick one backend"

Trying to name a single primary store is the wrong question (raised 2026-07-27).
Real setups differ wildly: **GPG-only, KeePassXC-only, both, or many** vaults at
once. So the destination must be **pluggable / user-choosable per run**, not a
baked-in default:

- Detect what's actually present (a GPG public key? a running Secret Service?
  `pass`? nothing?) and offer only those, plus the always-available warned
  plaintext.
- A "Save passwords to…" control lists the available sinks: **Encrypted file
  (GPG)** · **Password manager / Secret Service** (→ KeePassXC or Seahorse) ·
  **Import file (KeePassXC/Bitwarden CSV)** · **Plaintext (warned)**.
- Probably ship **one** backend first (whichever the discussion lands on) behind
  a trait/interface so the others slot in later without rework.

**Status: blocked on more discussion — do NOT build yet.** Open questions to
resolve first: which backend ships first; how to detect availability cleanly from
the sandbox; whether the manifest is per-run or an appended running log; and
whether Septima ever *reads* a store back (in-app password lookup / auto-unlock)
or only writes. Revisit when the user wants to design it properly.

### UX — two ways to land it

**A. Incremental — extend the batch-compress flow (smaller):**
In the create dialog's batch mode, add an encryption sub-choice:
`Password: [None] [One shared] [Generate one per archive]`. Picking "generate
per archive" reveals: password length/charset, and a **"Save passwords to…"**
manifest control (format = encrypted / CSV / plaintext-with-warning). Reuses the
existing per-item compress loop; the only new engine work is password generation
and manifest writing.

**B. Ambitious — a dedicated "Batch" view/tab (bigger):**
A power-user surface, distinct from the focused single-archive create dialog:
- A table of jobs: source dir → output archive → password (● generated /
  shared / none) → status.
- Global controls: format/codec/level applied to all, password policy, manifest
  destination, "test each archive after creating," optional "delete source
  after."
- Runs the whole batch with a progress row per job (we already do this).
- This is also the natural home for the `{source: destination}` mapping idea and
  for importing a job list from a file.

`A` is a few days on top of batch compress. `B` is a real new mode — worth it
only if batch is going to be a headline use case (the user's workflow suggests
it might be).

### Password generation details

- Source: OS CSPRNG (`getrandom` / `/dev/urandom`) — never a userspace PRNG.
- Configurable **length** (default 64) and **charset** (alphanumeric default;
  optionally add symbols — but note some contexts choke on shell-special chars,
  and Septima passes passwords as `7zz` args, so keep the default shell-safe).
- Passwords must **never** hit the debug job log — the engine already redacts
  `-p<...>` to `-p<redacted>`; verify that still holds on this path.

### Open questions

- Is the manifest **per-batch** (one file for the whole run) or appended to a
  **running log** across runs? (User said "dump into a log file… to open
  later" — sounds like an appended running log. That argues for a stable,
  chosen log location + append mode.)
- Manifest fields beyond name+password? (date, format, size, SHA-256 of the
  archive so the log doubles as an integrity record — nice synergy with the
  checksum feature.)
- Do we ever offer to **re-open the log inside Septima** to look a password up,
  or is the log purely external? (If encrypted-manifest, an in-app viewer is a
  natural pairing.)

### Refinement: the manifest FILE is the interchange (2026-07-27)

The vault-compatibility question collapses once you see it the user's way: **the
manifest is a portable file; the vault just stores that file; the file is what
Septima reads back to drive decryption.** Septima never talks to a vault's API.

- Compatibility with **KeePassXC · Bitwarden · pass · Seahorse** is inherent —
  they all store a file (attachment / entry / `pass insert -m` / a GPG file
  Seahorse's key wraps). The user puts it wherever; that's *their* vault's job.
- **Two artifacts, that's it:** the manifest CSV, and an *optional* GPG-encrypted
  copy of it for bare-disk / cloud storage (where no vault provides encryption).
- **Round-trip through the UI:** to decrypt, the user retrieves the file from
  their vault and opens it in Septima → Septima parses it → batch-decrypt runs.
  No libsecret, no D-Bus, no per-vault plugin for the core.

**What this unblocks:** the deferred "which backend" fight was mostly about *API
integration*. With file-as-interchange, the core needs **none** of it — just
read/write a (optionally GPG-encrypted) file. The only remaining crypto choice is
GPG-encrypt via bundled OpenPGP (B1, sandbox-clean) vs host-spawn (B2); plaintext
+ "store it in your vault yourself" works with zero crypto in Septima.

**Deep integrations become optional niceties, not core:** live Secret-Service
auto-unlock, or one-click "import to KeePassXC," can come *later* as conveniences
on top — they're no longer on the critical path.

### Manifest robustness — P1, must be flawless (2026-07-27)

A wrong manifest = unrecoverable archives, so every field and edge is pinned:

**Field spec (exactly):**
- `archive` — the archive's **basename**, never an absolute path. Absolute paths
  break the moment the manifest moves to another machine/dir (the whole point is
  portability). On decrypt, resolve relative to the manifest's folder or a
  user-picked archives dir.
- `source` — **informational only** (what it was made from); never used to
  decrypt. Safe if it's lossy.
- `password` — the exact secret, **never trimmed**, ASCII (our charsets).
- `sha256` — **optional** integrity; if present, verify before trusting/deleting.
- `created` — informational UTC ISO-8601 (the GTK layer fills this via glib).

**Edges and the rule for each:**
1. **Atomicity (the big one).** Append + flush each row the instant its archive
   is created — never leave an archive on disk whose password isn't yet persisted.
   A crash mid-batch must leave every already-made archive fully recoverable.
2. **Single point of failure.** Before ANY destructive step (delete-source,
   delete-archive-after), confirm the manifest is written *and* re-readable.
   Encourage the encrypted copy / a backup.
3. **CSV formula injection.** A password starting with `= + - @` runs as a formula
   if the plaintext CSV is opened in Excel/LibreOffice. The **default charset
   (Alphanumeric) is immune**; for AlphanumericSymbols, forbid a leading
   `= + - @` (regen the first char). [TODO in the generator when symbols land.]
4. **UTF-8 BOM.** Excel prepends one; stripped on read. ✅ done + tested.
5. **Non-UTF-8 filenames.** Linux allows arbitrary bytes; lossy conversion could
   mangle. Archive names are generated ASCII (safe); `source` is informational so
   lossy is acceptable there.
6. **Duplicate archive names → ambiguous decrypt.** The create flow must generate
   unique names (the historical prefix+seq did exactly this).
7. **Empty/whitespace password.** Never produced on create; on decrypt, skip +
   warn rather than fail silently. Never trim.
8. **Passwords never hit logs.** Engine already redacts `-p<…>`; never log a
   `Manifest`.
9. **Line endings / blanks.** CRLF, LF, bare CR all parse; blank lines skipped.
   File is written UTF-8, CRLF, no BOM. ✅ done + tested.

### Runtime crypto — it's ALL already in the GNOME 50 runtime (2026-07-27)

Verified inside the bundled runtime (corrects the "GPG isn't in the runtime"
assumption):
- **`/usr/bin/gpg`** — the real GnuPG binary.
- **gpgme / gpgmepp, libgcrypt, libassuan, libgpg-error** — GnuPG's programmatic
  stack.
- **libsecret** — the Secret Service client (talks to KeePassXC / gnome-keyring).
- **gcr** — GNOME crypto UI.

**So neither bundling nor a Flatpak extension is needed for GPG or Secret Service:**
- **GPG-encrypt the manifest** by shelling the runtime's `/usr/bin/gpg` (mirrors
  how we already drive `7zz`), encrypting to the user's **exported public key** →
  sandbox stays clean (only the pubkey, never `~/.gnupg`, no host-spawn). The user
  decrypts later with their normal host gpg/Seahorse.
- **Secret Service** is reachable via the already-present `libsecret` + the narrow
  `--talk-name=org.freedesktop.secrets` permission.
- **The "helper extension" idea** (add-extension) is still valid *if* we later want
  a backend to be an opt-in install — but it's not required. Simpler path: ship the
  support feature-gated, using the runtime libs that are already there.

### Future: GPG on the archives themselves (distant, but the runtime enables it)

Because `/usr/bin/gpg` is in the runtime, one "shell-gpg" helper (built first for the
manifest) unlocks a trajectory:

1. **Manifest GPG-encryption** — near-term, part of batch-encrypt. Builds the helper.
2. **`.gpg` / `.tar.gpg` encrypted archives** — future. `gpg -c` (symmetric,
   passphrase) *or* `gpg -e -r <recipient>` (to a public key). Both sandbox-clean
   (pubkey/passphrase only). Asymmetric is a **capability 7z lacks** — 7z encryption
   is passphrase-only, so "encrypt this archive *to Alice's public key*" needs GPG.
3. **GPG signing archives** (detached `.sig`/`.asc`) — distant. Needs the user's
   *secret* key + passphrase, i.e. host `~/.gnupg` access or a sandbox import — more
   invasive than encryption. Niche authenticity feature.

Each step reuses the same shell-gpg plumbing, so #1 lays the groundwork for #2/#3.

### Backend-agnostic design (agreed to spec now — 2026-07-27)

Everything below is independent of *which* password store ships first. The store
is a pluggable slot (the `ManifestSink` below); only its concrete backends wait
on the deferred discussion.

**1. Manifest schema (CSV, RFC-4180 quoted).** One row per archive, header row
first. Chosen to double as a password-manager import *and* an integrity record:

```
archive,source,password,sha256,created
photos.7z,photos/,<64-char pw>,3f9a…,2026-07-27T14:03:00Z
docs.7z,docs/,<64-char pw>,a17b…,2026-07-27T14:03:04Z
```

- `archive` also serves as the vault entry Title on import; `password` maps to
  Password. (KeePassXC/Bitwarden let you map columns, so extra columns are fine.)
- `sha256` lets a later run verify the archive is intact before trusting it —
  reuses the existing hash engine. Optional column, on by default.
- 64-char alphanumeric passwords never contain commas/quotes, so the common case
  needs no escaping; quote anyway for correctness.

**2. Create flow** (extends batch compress):
`stage items → password policy (none / one shared / generate-per-item{len,charset})
→ pick a sink → for each item: gen pw · compress to its own encrypted archive
(the dialog's tuning — -mhe for 7z, solid off, etc.) · sha256 it · append a row →
write the manifest to the sink → toast "N archives + manifest → <sink>".`

**3. Decrypt / read flow** (the loop the old decryptor scripts did):
`open a manifest (or read it from a sink) → parse archive↔password rows → batch-
extract each with its own password (no prompt), into sibling folders or a chosen
dest → optional sha256 verify → optional delete-after.` This is literally batch
extract with the password supplied per-archive from the manifest — reuses the
batch-extract machinery and the run_extract password path wholesale.

**4. The pluggable slot — `ManifestSink`:**

```
trait ManifestSink {
    fn write(&self, manifest: &Manifest) -> Result<Located>;   // where it landed
    fn read(&self)  -> Result<Manifest>;                        // None for write-only sinks
    fn is_available(&self) -> bool;                             // detected at runtime
    fn label(&self) -> &str;
}
```

Backends fill this in later (deferred decision): PlaintextFile(warned) ·
EncryptedFile(GPG-to-pubkey / age) · SecretService(→ KeePassXC/Seahorse) ·
PasswordManagerCsv(write-only export). The dialog's "Save passwords to…" lists
only `is_available()` sinks. Ship one, add the rest without touching the flows.

**5. Engine vs GTK split** (respect the A1 boundary — engine stays UI-free):
- **Engine:** password generation (`getrandom`), the `Manifest` type + CSV
  read/write, sha256 (exists), and the PlaintextFile / PasswordManagerCsv /
  EncryptedFile(if via bundled OpenPGP) sinks — all pure IO/crypto.
- **GTK / boundary:** SecretService sink (needs libsecret/D-Bus), the "Save
  passwords to…" UI, and availability detection that touches portals.

**Still TBD (the blocked bits):** which sink ships first; sandbox-clean
availability detection; per-run manifest vs an appended running log; and whether
GPG uses bundled OpenPGP (B1, sandbox-clean) or host-spawn (B2). None of these
block writing the engine-side `Manifest` + generator + CSV + plaintext sink,
which is the natural first slice.

### ✅ DESIGN SETTLED (2026-08-11) — supersedes the deferred sections above

**Built the same day, on the `batch-encrypt` branch** — write side (dialog
controls, passwords-first ordering, serialized atomic rewrites) and read side
(open a `.json`/`.json.gpg`/`.csv` → batch-extract with recorded passwords),
with `real_manifest_flow.rs` covering the loop end to end. Interactive UI
testing is what remains before it ships.

Talked through with the user; these are decisions, not options:

1. **The manifest is JSON, not CSV.** Human review is the primary use — the user
   opens it, reads it, pastes entries into their password manager of choice.
   Self-describing keys beat columns for that, and the Excel formula-injection
   edge disappears. The shipped CSV code is repurposed as an optional
   **"Export as CSV"** action for KeePassXC/Bitwarden bulk import (they import
   CSV, not arbitrary JSON) — the leading `=+-@` rule still applies there.
2. **Shape: one file per batch run** (`septima-passwords-<date>.json`, portal-
   picked destination). No appended running log — "keep the files in one folder"
   replaces it, and per-run files work cleanly with encryption.
3. **Schema:** top-level `{version, septima, created}` header + `entries[]` of
   `{archive, source, password, sha256, created, encryption}` — `encryption` is
   the human-readable cipher note ("7z, AES-256, encrypted headers"), the thing
   nobody remembers six months later. Field rules from "Manifest robustness"
   above all still hold (basename-only archive, never trim password, etc.).
4. **Protection is symmetric GPG only** (`gpg -c` via the runtime's
   `/usr/bin/gpg`): no keys, no keyring, no host-spawn, decrypts anywhere.
   **Signing is parked indefinitely** — it needs the secret key, i.e. host
   `~/.gnupg` access, and buys little for a self-produced file.
5. **The encrypt-or-not ask lives in the batch dialog, before the run** — a
   "Passwords file" control: Plain JSON / Password-protected (`.json.gpg`).
   Deciding up front means an encrypted run never leaves plaintext residue on
   disk, and crash-safety works in both modes (manifest rewritten atomically —
   temp + rename — after every archive completes; the passphrase is already in
   hand for encrypted mode). Plaintext default is acceptable *because* the file
   is transient (reviewed → vaulted → deleted), but declining protection shows
   a plain-words warning about leaving the file next to the archives.

### Phased plan (if pursued)

1. Password generator (engine) + redaction check.
2. Per-archive password in the batch-compress loop (UX approach A).
3. Manifest writing: start with the **encrypted-manifest** default + a clearly
   warned plaintext option.
4. Later: password-manager CSV export; then keyring + auto-unlock (approach B
   territory).

### My honest take

The workflow is legit and Septima is unusually well-placed to do it well
*because* it can pair generation with the encrypted-manifest and the existing
checksum/verify features — turning a fragile shell loop into an audited,
one-shot batch. The one hard rule: **do not ship a feature whose easy path is a
plaintext file full of passwords next to the archives.** Make the secure store
the default, plaintext the warned exception, and this becomes a genuinely
differentiating power-user feature rather than a footgun.
