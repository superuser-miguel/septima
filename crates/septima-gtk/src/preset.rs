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
    pub bcj: bool,
    pub encrypt_headers: bool,
    pub extra_params: Vec<String>,
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
            bool_str(self.bcj),
            bool_str(self.encrypt_headers),
            self.extra_params.join(" "),
        ];
        fields.join(&SEP.to_string())
    }

    fn deserialize(s: &str) -> Option<Preset> {
        let f: Vec<&str> = s.split(SEP).collect();
        if f.len() != 11 || f[0].is_empty() {
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
            bcj: f[8] == "1",
            encrypt_headers: f[9] == "1",
            extra_params: f[10].split_whitespace().map(str::to_string).collect(),
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
