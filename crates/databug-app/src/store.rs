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
    /// The latest error saving something to disk, until the app shows it.
    error: std::cell::RefCell<Option<String>>,
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
        // engine libraries downloaded on demand (DuckDB)
        databug_drivers::set_runtime_dir(data_dir.join("runtimes"));
        let store = Store { config_dir: config_dir.clone(), data_dir: data_dir.clone(), memory_secrets: HashMap::new(), error: Default::default() };
        for dir in [&config_dir, &data_dir.join("catalogs")] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                store.report(&format!("Couldn't create {}", dir.display()), &e);
            }
        }
        store
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, path: PathBuf) -> Option<T> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Remember a failed save so the app can tell the user.
    fn report(&self, what: &str, e: &dyn std::fmt::Display) {
        *self.error.borrow_mut() = Some(format!("{what}: {e}"));
    }

    /// The latest save error, if any (cleared once taken).
    pub fn take_error(&self) -> Option<String> {
        self.error.borrow_mut().take()
    }

    /// Write JSON atomically: to a temporary file, then rename over the old one.
    fn write_json<T: Serialize>(&self, path: PathBuf, value: &T) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match serde_json::to_string_pretty(value) {
            Ok(t) => t,
            Err(e) => return self.report(&format!("Couldn't save {name}"), &e),
        };
        let tmp = path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            self.report(&format!("Couldn't save {name} (your change is only in memory)"), &e);
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
        match serde_json::to_string(cat) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    self.report("Couldn't save the schema cache", &e);
                }
            }
            Err(e) => self.report("Couldn't save the schema cache", &e),
        }
    }

    pub fn delete_catalog(&self, id: &str) {
        match std::fs::remove_file(self.data_dir.join("catalogs").join(format!("{id}.json"))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => self.report("Couldn't delete the schema cache", &e),
        }
    }

    pub fn load_history(&self, max: usize) -> Vec<HistoryEntry> {
        let Ok(f) = std::fs::File::open(self.data_dir.join("history.jsonl")) else { return vec![] };
        let mut v: Vec<HistoryEntry> = std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
        if v.len() > max {
            v.drain(..v.len() - max);
        }
        v
    }

    /// Add a statement to the history file, with passwords masked.
    pub fn append_history(&self, e: &HistoryEntry) {
        let e = HistoryEntry { sql: redact_secrets(&e.sql), ..e.clone() };
        let res = serde_json::to_string(&e).map_err(std::io::Error::other).and_then(|line| {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.data_dir.join("history.jsonl"))?;
            writeln!(f, "{line}")
        });
        if let Err(err) = res {
            self.report("Couldn't save query history", &err);
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

/// Mask passwords and secrets inside SQL before it's written to disk:
/// `PASSWORD 'x'`, `PASSWORD = 'x'`, `IDENTIFIED BY x`, `SECRET = 'x'`,
/// and MongoDB's `pwd: "x"`.
pub fn redact_secrets(sql: &str) -> String {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let pats = PATTERNS.get_or_init(|| {
        let quoted = r#"'(?:[^']|'')*'|"(?:[^"]|"")*"|N'(?:[^']|'')*'"#;
        [
            format!(r"(?i)(\b(?:PASSWORD|SECRET)\s*(?:=|\s)\s*)(?:{quoted})"),
            format!(r"(?i)(\bIDENTIFIED\s+(?:WITH\s+\S+\s+)?BY\s+(?:PASSWORD\s+)?)(?:{quoted}|[^\s;]+)"),
            format!(r#"(?i)(\bpwd\s*:\s*)(?:{quoted})"#),
        ]
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect()
    });
    let mut out = sql.to_string();
    for re in pats {
        out = re.replace_all(&out, "${1}'***'").into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_masked_in_history() {
        assert_eq!(redact_secrets("ALTER USER app WITH PASSWORD 'hunter2';"), "ALTER USER app WITH PASSWORD '***';");
        assert_eq!(redact_secrets("CREATE LOGIN app WITH PASSWORD = N'p''w', CHECK_POLICY = OFF"), "CREATE LOGIN app WITH PASSWORD = '***', CHECK_POLICY = OFF");
        assert_eq!(redact_secrets("create user 'a'@'%' identified by 'pw1'"), "create user 'a'@'%' identified by '***'");
        assert_eq!(redact_secrets("ALTER USER a IDENTIFIED WITH caching_sha2_password BY 'x'"), "ALTER USER a IDENTIFIED WITH caching_sha2_password BY '***'");
        assert_eq!(redact_secrets("CREATE USER scott IDENTIFIED BY tiger"), "CREATE USER scott IDENTIFIED BY '***'");
        assert_eq!(redact_secrets("CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = 'abc'"), "CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = '***'");
        assert_eq!(redact_secrets(r#"db.createUser({user: "a", pwd: "s3cret", roles: []})"#), r#"db.createUser({user: "a", pwd: '***', roles: []})"#);
        // ordinary SQL is untouched
        let q = "SELECT password_hash FROM users WHERE name = 'password'";
        assert_eq!(redact_secrets(q), q);
    }

    #[test]
    fn failed_saves_are_reported() {
        let dir = std::env::temp_dir().join(format!("databug-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a file where the config directory should be makes every save fail
        let blocker = dir.join("config");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let store = Store { config_dir: blocker, data_dir: dir.join("data"), memory_secrets: HashMap::new(), error: Default::default() };
        store.save_settings(&Settings::default());
        let err = store.take_error().expect("the failed save is reported");
        assert!(err.contains("settings.json"), "{err}");
        assert!(store.take_error().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
