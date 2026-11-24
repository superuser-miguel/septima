//! Every `7zz` command and switch Septima can send — the source of truth for the
//! app's "What Septima Can Do" dialog.
//!
//! The command builders (`add_command`, `extract_command`, …) sit in their own
//! modules; a test here builds each of them with every option switched on and
//! checks that the arguments they produce and this table agree exactly. A switch
//! the engine starts sending without an entry here, or an entry for something
//! it no longer sends, fails the build, so the dialog can't drift from what the
//! app really does.
//!
//! The Advanced field's free-form switches are not entries: they are the escape
//! hatch for everything without a control (see [`ADVANCED_EXAMPLES`]).

/// Which part of the app a switch belongs to, for grouping in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Create,
    Extract,
    Edit,
    Plumbing,
}

impl Group {
    /// Dialog order.
    pub const ORDER: [Group; 4] = [Group::Create, Group::Extract, Group::Edit, Group::Plumbing];
}

/// How a switch in the table is matched against the arguments sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// The argument is exactly this text.
    Exact,
    /// The argument starts with this text, and a value follows (`-mx=9`, `-p…`).
    Prefix,
}

/// One `7zz` command (the first argument).
pub struct Command {
    /// The command letter(s); `""` is `7zz` run with no arguments.
    pub name: &'static str,
    pub title: &'static str,
    /// What in the app runs it.
    pub used_by: &'static str,
}

/// One switch.
pub struct Switch {
    pub switch: &'static str,
    pub matching: Match,
    pub title: &'static str,
    /// The control that sets it, or when it's sent.
    pub control: &'static str,
    pub description: &'static str,
    pub group: Group,
    /// `false` = the engine can send it, but nothing in the app asks for it
    /// yet. Listed so the dialog can say so, instead of hiding it.
    pub offered: bool,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "a", title: "Create an archive", used_by: "New Archive, batch compress and batch encrypt" },
    Command { name: "x", title: "Extract", used_by: "Extract, drag-out, batch extract and opening a passwords file" },
    Command { name: "l", title: "List the contents", used_by: "Opening an archive" },
    Command { name: "t", title: "Test integrity", used_by: "Test Archive" },
    Command { name: "d", title: "Delete entries", used_by: "Delete Entries" },
    Command { name: "rn", title: "Rename an entry", used_by: "Rename Entry" },
    Command { name: "h", title: "Calculate checksums", used_by: "Calculate Checksums, and the checksum file for a new archive" },
    Command { name: "i", title: "List supported formats and codecs", used_by: "At start, to see which ciphers this 7zz offers" },
    Command { name: "", title: "Show its name and version", used_by: "About → Troubleshooting" },
];

const fn s(
    switch: &'static str,
    matching: Match,
    title: &'static str,
    control: &'static str,
    description: &'static str,
    group: Group,
) -> Switch {
    Switch { switch, matching, title, control, description, group, offered: true }
}

use Group::*;
use Match::*;

pub const SWITCHES: &[Switch] = &[
    // Creating
    s("-t", Prefix, "Format", "Format",
      "7z, zip or tar, or a single-file stream such as zst or br. Also used inside the two steps of a compressed tarball.", Create),
    s("-m0=", Prefix, "Method", "Method, or Filter",
      "The compression method. When a filter is chosen, the filter goes here and the method moves to -m1=.", Create),
    s("-m1=", Prefix, "Method after a filter", "Filter",
      "The compression method, when a filter (BCJ, ARM64, Delta, …) runs before it.", Create),
    s("-mm=", Prefix, "Zip method", "Method",
      "Zip's own key for the compression method.", Create),
    s("-mx=", Prefix, "Level", "Level, and Lizard family",
      "How hard to compress. For Lizard, the family and level combine into one number.", Create),
    s("-md=", Prefix, "Dictionary size", "Dictionary size",
      "How far back LZMA, LZMA2 and Fast-LZMA2 look for repeats. Bigger can compress better and needs more memory.", Create),
    s("-m1d=", Prefix, "Dictionary size, after a filter", "Dictionary size",
      "The same, aimed at the method when a filter comes first.", Create),
    s("-mmt=", Prefix, "Threads", "CPU threads",
      "How many CPU threads to use. Also sent as on or off when retrying a brotli file that didn't open (see Good to know).", Create),
    s("-ms=", Prefix, "Solid archive", "Solid archive",
      "Compress all files as one stream: smaller, but slower to update or to open a single file.", Create),
    s("-p", Prefix, "Password", "Password, unique passwords, or a password prompt",
      "The archive's password. Also sent when opening, testing or editing an encrypted archive.", Create),
    s("-mhe=on", Exact, "Encrypt file names", "Encrypt file names",
      "Hides the list of files in a 7z archive until the password is given.", Create),
    s("-mem=", Prefix, "Encryption method", "Encryption",
      "Picks the cipher: AES-256 for zip instead of its weak ZipCrypto default, or one of the 7-Zip ZS ciphers for 7z instead of AES-256.", Create),
    s("-v", Prefix, "Split into volumes", "Split into volumes",
      "Writes the archive in parts of the chosen size: name.7z.001, .002, …", Create),
    // Extracting and opening
    s("-o", Prefix, "Destination folder", "The folder you extract to", "Where extracted files go.", Extract),
    s("-aou", Exact, "Keep both", "Keep both, when files would clash; also the default",
      "Files already in the destination stay; the incoming ones get a number, like report_1.txt. \
       Used for every extract unless you pick otherwise, so nothing is replaced by surprise.", Extract),
    s("-aos", Exact, "Skip existing files", "Skip, when files would clash",
      "Files already in the destination stay, and the archive's copies of them aren't extracted.", Extract),
    s("-aoa", Exact, "Replace existing files", "Replace, when files would clash",
      "Files already in the destination with the same name are replaced.", Extract),
    Switch { offered: false, ..s("-aot", Exact, "Rename existing files", "Not offered",
      "Move the files already there to new names, and extract in their place.", Extract) },
    s("-slt", Exact, "Detailed listing", "Opening an archive",
      "Lists every entry with all its details, which Septima reads to fill the file list.", Extract),
    s("-so", Exact, "Write to a pipe", "Compressed tarballs",
      "The outer step of a .tar.zst and friends: decompress into a pipe instead of a file.", Extract),
    s("-si", Exact, "Read from a pipe", "Compressed tarballs",
      "The inner step: read the tar from that pipe, so the intermediate tar never lands on disk.", Extract),
    // Editing and checking
    s("-scrc", Prefix, "Checksum type", "Calculate Checksums",
      "Which digests to compute: CRC32, SHA256, BLAKE3 and the rest.", Edit),
    // How Septima talks to 7zz
    s("-bsp1", Exact, "Progress", "Every long job", "Reports progress, which drives Septima's progress bars.", Plumbing),
    s("-bsp2", Exact, "Progress, out of the way", "Compressed tarballs",
      "The same, sent where it doesn't mix with the data in the pipe.", Plumbing),
    s("-bb1", Exact, "File names", "Every long job", "Names each file as it's handled, shown under the progress bar.", Plumbing),
    s("-y", Exact, "No questions", "Every job that writes",
      "7zz never stops to ask on the command line; Septima asks you in the window instead.", Plumbing),
];

/// Common switches with no control, for the Advanced field when creating an
/// archive. Shown as examples, not a promise: anything 7zz accepts goes.
pub const ADVANCED_EXAMPLES: &[(&str, &str)] = &[
    ("-mfb=273", "Word size for LZMA and LZMA2"),
    ("-mmf=bt4", "Match finder for LZMA and LZMA2"),
    ("-mo=32", "Model order for PPMd"),
    ("-ms=100m", "Solid block size, instead of one solid block"),
    ("-snl", "Store symbolic links as links"),
    ("-mtc=on", "Store creation times"),
];

/// The table entry that `arg` (a single argument) matches, if any. An exact
/// entry wins over a prefix one; among prefixes, the longest wins.
pub fn find(arg: &str) -> Option<&'static Switch> {
    SWITCHES
        .iter()
        .filter(|s| match s.matching {
            Exact => arg == s.switch,
            Prefix => arg.starts_with(s.switch) && arg.len() > s.switch.len(),
        })
        .max_by_key(|s| (s.matching == Exact, s.switch.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::{add_command, CompressionRequest};
    use crate::extract::{decompress_tar_command, extract_command, untar_command, ExtractRequest, OverwriteMode};
    use std::collections::BTreeSet;
    use std::path::Path;

    fn args(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    /// Every command the engine can build, with every option it has switched
    /// on, so every switch it could ever send appears at least once. Requests
    /// are built field by field: a new option shows up here as a compile error
    /// or an unmatched switch, which is the point.
    fn every_command() -> Vec<Vec<String>> {
        let z = Path::new("7zz");
        let a = Path::new("a.7z");
        let mut out = Vec::new();

        // Create: 7z with a filter and every option on.
        let mut req = CompressionRequest::new("a.7z", vec!["in".into()], "7z");
        req.codec = Some("lzma2".into());
        req.level = Some(9);
        req.threads = Some(4);
        req.dictionary = Some("64m".into());
        req.solid = Some(true);
        req.volume_size = Some("100m".into());
        req.filter = Some("ARM64".into());
        req.password = Some("pw".into());
        req.encrypt_headers = true;
        req.encryption_method = Some("AES256".into());
        out.push(args(&add_command(z, &req)));
        // Without a filter, so -m0= carries the method and -md= the dictionary.
        req.filter = None;
        out.push(args(&add_command(z, &req)));
        // Zip uses its own method key.
        let mut zip = CompressionRequest::new("a.zip", vec!["in".into()], "zip");
        zip.codec = Some("deflate".into());
        out.push(args(&add_command(z, &zip)));
        // A raw stream: the codec is the format.
        let mut stream = CompressionRequest::new("a.br", vec!["in".into()], "brotli");
        stream.level = Some(5);
        out.push(args(&add_command(z, &stream)));

        // Extract, in every overwrite mode, with a password and the brotli retry.
        for mode in [
            OverwriteMode::Overwrite,
            OverwriteMode::Skip,
            OverwriteMode::RenameExtracted,
            OverwriteMode::RenameExisting,
        ] {
            let mut ex = ExtractRequest::new("a.7z", "dest");
            ex.password = Some("pw".into());
            ex.overwrite = mode;
            ex.entries = vec!["one".into()];
            out.push(args(&extract_command(z, &ex, Some("-mmt=off"))));
            out.push(args(&untar_command(z, &ex)));
        }
        for progress in [true, false] {
            out.push(args(&decompress_tar_command(z, a, Some("-mmt=on"), progress)));
            out.push(args(&crate::hash::hash_command(z, a, &["SHA256", "CRC32"], progress)));
        }

        // List, test, edit, probe.
        out.push(args(&crate::command::list_command(z, a, Some("pw"), Some("-mmt=off"))));
        out.push(args(&crate::command::list_tar_stdin_command(z)));
        out.push(args(&crate::edit::test_command(z, a, Some("pw"))));
        out.push(args(&crate::edit::delete_command(z, a, &["one".into()], Some("pw"))));
        out.push(args(&crate::edit::rename_command(z, a, &[("one".into(), "two".into())], Some("pw"))));
        out.push(args(&crate::capabilities::info_command(z)));
        out.push(args(&crate::command::banner_command(z)));
        out
    }

    /// The first argument, or `""` for a bare `7zz`.
    fn command_name(argv: &[String]) -> &str {
        argv.first().map(String::as_str).unwrap_or("")
    }

    #[test]
    fn every_command_sent_is_listed_and_every_listed_one_is_sent() {
        let all = every_command();
        let sent: BTreeSet<String> = all.iter().map(|a| command_name(a).to_string()).collect();
        let listed: BTreeSet<String> = COMMANDS.iter().map(|c| c.name.to_string()).collect();
        assert_eq!(sent, listed, "switches::COMMANDS drifted from the command builders");
    }

    #[test]
    fn every_switch_sent_is_listed_and_every_listed_one_is_sent() {
        let mut used = BTreeSet::new();
        for argv in every_command() {
            for arg in argv.iter().skip(1).filter(|a| a.starts_with('-') && *a != "--") {
                let hit = find(arg).unwrap_or_else(|| panic!("7zz is sent {arg}, which switches::SWITCHES doesn't list"));
                used.insert(hit.switch);
            }
        }
        for sw in SWITCHES {
            assert!(used.contains(sw.switch), "switches::SWITCHES lists {}, which no command sends", sw.switch);
        }
    }

    #[test]
    fn exact_matches_win_and_prefixes_need_a_value() {
        assert_eq!(find("-mhe=on").unwrap().switch, "-mhe=on");
        assert_eq!(find("-mmt=4").unwrap().switch, "-mmt=");
        assert_eq!(find("-m1d=64m").unwrap().switch, "-m1d=");
        assert_eq!(find("-scrcSHA256").unwrap().switch, "-scrc");
        assert!(find("-p").is_none(), "a bare -p would be a password prompt, not a password");
    }
}
