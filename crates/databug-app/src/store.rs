//! Everything DataBug keeps on disk. Connections and settings are plain JSON
//! (safe to commit to Git); passwords go to the OS keychain; catalogs and
//! history live in the data directory.

use crate::theme::ThemeKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use databug_core::{Catalog, ConnectionConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeKind,
    pub code_font_size: f32,
    /// Interface zoom: 1.0 = 100%. Scales all text and controls.
    #[serde(default = "one")]
    pub ui_scale: f32,
    pub row_limit: usize,
    pub explorer_width: f32,
    pub auto_complete: bool,
    /// Color overrides per theme: theme → (color key → RGB).
    pub custom_colors: BTreeMap<String, BTreeMap<String, [u8; 3]>>,
    /// Project shown in the explorer; `None` shows every connection.
    pub current_project: Option<String>,
    /// Look for a newer release on GitHub at startup and once a day.
    #[serde(default = "yes")]
    pub check_updates: bool,
    /// Unix seconds of the last update check.
    pub last_update_check: u64,
    /// A release the user chose to skip (its tag).
    pub skipped_version: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeKind::Dark,
            code_font_size: 13.5,
            ui_scale: 1.0,
            row_limit: 10_000,
            explorer_width: 270.0,
            auto_complete: true,
            custom_colors: BTreeMap::new(),
            current_project: None,
            check_updates: true,
            last_update_check: 0,
            skipped_version: None,
        }
    }
}

impl Settings {
    pub fn theme_key(&self) -> String {
        format!("{:?}", self.theme)
    }

    pub fn overrides(&self) -> BTreeMap<String, [u8; 3]> {
        self.custom_colors.get(&self.theme_key()).cloned().unwrap_or_default()
    }
}

/// A named set of connections. The explorer shows the connections of the
/// current project, in the project's order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub connections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: u64,
    pub connection: String,
    pub sql: String,
    pub ms: u64,
    pub ok: bool,
}

pub struct Store {
    config_dir: PathBuf,
    data_dir: PathBuf,
    /// Passwords for this session when the keychain isn't available or the
    /// user chose not to save them.
    memory_secrets: HashMap<String, String>,
}

impl Store {
    pub fn open() -> Self {
        let (config_dir, data_dir) = match std::env::var_os("DATABUG_HOME") {
            Some(h) => {
                let h = PathBuf::from(h);
                (h.join("config"), h.join("data"))
            }
            None => match directories::ProjectDirs::from("dev", "DataBug", "DataBug") {
                Some(d) => (d.config_dir().to_path_buf(), d.data_dir().to_path_buf()),
                None => (PathBuf::from(".databug/config"), PathBuf::from(".databug/data")),
            },
        };
        let _ = std::fs::create_dir_all(&config_dir);
        let _ = std::fs::create_dir_all(data_dir.join("catalogs"));
        // engine libraries downloaded on demand (DuckDB)
        databug_drivers::set_runtime_dir(data_dir.join("runtimes"));
        Store { config_dir, data_dir, memory_secrets: HashMap::new() }
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, path: PathBuf) -> Option<T> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_json<T: Serialize>(&self, path: PathBuf, value: &T) {
        if let Ok(text) = serde_json::to_string_pretty(value) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    pub fn load_connections(&self) -> Vec<ConnectionConfig> {
        self.read_json(self.config_dir.join("connections.json")).unwrap_or_default()
    }

    pub fn save_connections(&self, conns: &[ConnectionConfig]) {
        self.write_json(self.config_dir.join("connections.json"), &conns);
    }

    pub fn load_projects(&self) -> Vec<Project> {
        self.read_json(self.config_dir.join("projects.json")).unwrap_or_default()
    }

    pub fn save_projects(&self, projects: &[Project]) {
        self.write_json(self.config_dir.join("projects.json"), &projects);
    }

    pub fn load_settings(&self) -> Settings {
        self.read_json(self.config_dir.join("settings.json")).unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) {
        self.write_json(self.config_dir.join("settings.json"), s);
    }

    pub fn load_catalog(&self, id: &str) -> Option<Catalog> {
        self.read_json(self.data_dir.join("catalogs").join(format!("{id}.json")))
    }

    pub fn save_catalog(&self, id: &str, cat: &Catalog) {
        let path = self.data_dir.join("catalogs").join(format!("{id}.json"));
        if let Ok(text) = serde_json::to_string(cat) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn delete_catalog(&self, id: &str) {
        let _ = std::fs::remove_file(self.data_dir.join("catalogs").join(format!("{id}.json")));
    }

    pub fn load_history(&self, max: usize) -> Vec<HistoryEntry> {
        let Ok(f) = std::fs::File::open(self.data_dir.join("history.jsonl")) else { return vec![] };
        let mut v: Vec<HistoryEntry> = std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
        if v.len() > max {
            v.drain(..v.len() - max);
        }
        v
    }

    pub fn append_history(&self, e: &HistoryEntry) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.data_dir.join("history.jsonl")) {
            if let Ok(line) = serde_json::to_string(e) {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    // ---- secrets ----

    pub fn password(&self, id: &str) -> Option<String> {
        if let Some(p) = self.memory_secrets.get(id) {
            return Some(p.clone());
        }
        if std::env::var_os("DATABUG_NO_KEYRING").is_some() {
            return None;
        }
        keyring::Entry::new("databug", id).ok()?.get_password().ok()
    }

    /// Store a password. Returns false when it could only be kept in memory.
    pub fn set_password(&mut self, id: &str, password: &str, persist: bool) -> bool {
        self.memory_secrets.insert(id.to_string(), password.to_string());
        if !persist || std::env::var_os("DATABUG_NO_KEYRING").is_some() {
            return !persist;
        }
        match keyring::Entry::new("databug", id) {
            Ok(e) => e.set_password(password).is_ok(),
            Err(_) => false,
        }
    }

    pub fn forget_password(&mut self, id: &str) {
        self.memory_secrets.remove(id);
        if std::env::var_os("DATABUG_NO_KEYRING").is_none() {
            if let Ok(e) = keyring::Entry::new("databug", id) {
                let _ = e.delete_credential();
            }
        }
    }
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}
