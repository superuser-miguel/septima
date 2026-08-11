use gtk::gio;
use gtk::prelude::*;

const KEY: &str = "compression-presets";
const SEP: char = '\u{1f}'; // unit separator between a preset's fields

/// A saved set of compression settings (never a password).
#[derive(Debug, Clone)]
pub struct Preset {
    pub name: String,
    pub format: String,
    pub codec: String,
    pub level: Option<u8>,
    pub threads: u32,
    pub dictionary: Option<String>,
    pub solid: Option<bool>,
    pub volume_size: Option<String>,
    /// Pre-codec filter method name (`BCJ`, `ARM64`, `Delta`, …); `None` = off.
    pub filter: Option<String>,
    pub encrypt_headers: bool,
    pub extra_params: Vec<String>,
    /// Cipher (`-mem=`) id; `None` = the format's default entry.
    pub encryption_method: Option<String>,
    pub write_checksum: bool,
    pub batch_mode: bool,
    /// Batch: a generated password per archive, recorded in a passwords file.
    pub generate_passwords: bool,
    /// Passwords file mode: GPG-protected. The passphrase itself is a secret
    /// and is never part of a preset — only the choice is.
    pub manifest_protected: bool,
}

impl Preset {
    fn serialize(&self) -> String {
        let fields = [
            self.name.clone(),
            self.format.clone(),
            self.codec.clone(),
            self.level.map(|l| l.to_string()).unwrap_or_default(),
            self.threads.to_string(),
            self.dictionary.clone().unwrap_or_default(),
            opt_bool(self.solid),
            self.volume_size.clone().unwrap_or_default(),
            self.filter.clone().unwrap_or_default(),
            bool_str(self.encrypt_headers),
            self.extra_params.join(" "),
            self.encryption_method.clone().unwrap_or_default(),
            bool_str(self.write_checksum),
            bool_str(self.batch_mode),
            bool_str(self.generate_passwords),
            bool_str(self.manifest_protected),
        ];
        fields.join(&SEP.to_string())
    }

    fn deserialize(s: &str) -> Option<Preset> {
        let f: Vec<&str> = s.split(SEP).collect();
        // 11 fields = a pre-0.5 preset; the newer fields default off. Anything
        // shorter is corrupt.
        if f.len() < 11 || f[0].is_empty() {
            return None;
        }
        Some(Preset {
            name: f[0].to_string(),
            format: f[1].to_string(),
            codec: f[2].to_string(),
            level: f[3].parse().ok(),
            threads: f[4].parse().unwrap_or(1),
            dictionary: non_empty(f[5]),
            solid: parse_opt_bool(f[6]),
            volume_size: non_empty(f[7]),
            // field[8] used to be a bcj bool ("0"/"1"); now it holds the filter
            // method name. Migrate a legacy "1" to the BCJ filter it stood for.
            filter: match f[8] {
                "" | "0" => None,
                "1" => Some("BCJ".to_string()),
                other => Some(other.to_string()),
            },
            encrypt_headers: f[9] == "1",
            extra_params: f[10].split_whitespace().map(str::to_string).collect(),
            encryption_method: f.get(11).and_then(|s| non_empty(s)),
            write_checksum: f.get(12).is_some_and(|s| *s == "1"),
            batch_mode: f.get(13).is_some_and(|s| *s == "1"),
            generate_passwords: f.get(14).is_some_and(|s| *s == "1"),
            manifest_protected: f.get(15).is_some_and(|s| *s == "1"),
        })
    }
}

/// GSettings-backed preset storage. Degrades gracefully to a no-op when the
/// schema isn't installed (e.g. a plain `cargo run` without `GSETTINGS_SCHEMA_DIR`),
/// since `gio::Settings::new` would otherwise abort.
pub struct PresetStore {
    settings: Option<gio::Settings>,
}

impl PresetStore {
    pub fn new() -> Self {
        let settings = gio::SettingsSchemaSource::default()
            .and_then(|src| src.lookup(crate::config::APP_ID, true))
            .map(|_| gio::Settings::new(crate::config::APP_ID));
        Self { settings }
    }

    pub fn is_available(&self) -> bool {
        self.settings.is_some()
    }

    pub fn list(&self) -> Vec<Preset> {
        let Some(settings) = &self.settings else {
            return Vec::new();
        };
        settings
            .strv(KEY)
            .iter()
            .filter_map(|s| Preset::deserialize(s))
            .collect()
    }

    /// Save `preset`, replacing any existing one with the same name.
    pub fn save(&self, preset: Preset) {
        let mut list = self.list();
        list.retain(|p| p.name != preset.name);
        list.push(preset);
        self.write(&list);
    }

    pub fn delete(&self, name: &str) {
        let list: Vec<Preset> = self.list().into_iter().filter(|p| p.name != name).collect();
        self.write(&list);
    }

    fn write(&self, list: &[Preset]) {
        if let Some(settings) = &self.settings {
            let serialized: Vec<String> = list.iter().map(Preset::serialize).collect();
            let _ = settings.set_strv(KEY, serialized);
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn bool_str(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

fn opt_bool(b: Option<bool>) -> String {
    match b {
        Some(true) => "1",
        Some(false) => "0",
        None => "",
    }
    .to_string()
}

fn parse_opt_bool(s: &str) -> Option<bool> {
    match s {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_field() {
        let p = Preset {
            name: "vault batch".into(),
            format: "7z".into(),
            codec: "lzma2".into(),
            level: Some(9),
            threads: 12,
            dictionary: Some("16m".into()),
            solid: Some(true),
            volume_size: None,
            filter: Some("BCJ".into()),
            encrypt_headers: true,
            extra_params: vec!["-mfb=273".into()],
            encryption_method: Some("AES256GCM".into()),
            write_checksum: true,
            batch_mode: true,
            generate_passwords: true,
            manifest_protected: true,
        };
        let back = Preset::deserialize(&p.serialize()).unwrap();
        assert_eq!(back.encryption_method.as_deref(), Some("AES256GCM"));
        assert!(back.write_checksum && back.batch_mode);
        assert!(back.generate_passwords && back.manifest_protected);
        assert_eq!(back.name, p.name);
        assert_eq!(back.filter, p.filter);
    }

    #[test]
    fn reads_a_pre_05_eleven_field_preset() {
        // Saved by 0.4.x: exactly 11 fields, no cipher/checksum/batch columns.
        let legacy = ["old", "zip", "deflate", "6", "4", "", "", "", "", "0", ""]
            .join(&SEP.to_string());
        let p = Preset::deserialize(&legacy).unwrap();
        assert_eq!(p.name, "old");
        assert_eq!(p.encryption_method, None);
        assert!(!p.write_checksum && !p.batch_mode && !p.generate_passwords);
    }
}
