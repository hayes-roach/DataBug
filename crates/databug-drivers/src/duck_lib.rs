//! DuckDB isn't compiled into DataBug. Its official shared library
//! (libduckdb) is downloaded on request from DuckDB's GitHub releases,
//! checked against the SHA-256 pinned below, unpacked into DataBug's data
//! folder and loaded at run time. `DATABUG_DUCKDB_LIB` points at a library
//! of your own instead.

use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The DuckDB release DataBug downloads.
pub const DUCKDB_VERSION: &str = "2.0.0";

/// One platform's download: the release asset, its SHA-256 and the library inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    pub file: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub library: &'static str,
}

/// The asset for this platform, if DuckDB publishes one.
pub fn asset() -> Option<Asset> {
    asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn asset_for(os: &str, arch: &str) -> Option<Asset> {
    // From https://github.com/duckdb/duckdb/releases/tag/v2.0.0 (GitHub's published digests).
    // Since 2.0 the library ships as duckdb-shared-libs-<platform>.tar.gz.
    let a = |file, sha256, size, library| Some(Asset { file, sha256, size, library });
    match (os, arch) {
        ("macos", _) => a("duckdb-shared-libs-osx-universal.tar.gz", "14cd63180614cfd961f7959146ecd9087527ff20b67311efdc3fa1c29c18204d", 40_361_654, "libduckdb.dylib"),
        ("linux", "x86_64") => a("duckdb-shared-libs-linux-amd64.tar.gz", "993ad89e7dfca66cd6da994c2cad090c2fc160ddf9f550498a098ab4bb13cfb1", 27_841_656, "libduckdb.so"),
        ("linux", "aarch64") => a("duckdb-shared-libs-linux-arm64.tar.gz", "c60eecf91143ca6bbac55c4f4d40dc22520488b0ac257c86e0fe9a2b451cf2b7", 25_735_927, "libduckdb.so"),
        ("windows", "x86_64") => a("duckdb-shared-libs-windows-amd64.tar.gz", "2b83169128239de5f45fe21de80f99bf436e53aa4bb24020dd951a1ba8d4b95f", 19_833_650, "duckdb.dll"),
        ("windows", "aarch64") => a("duckdb-shared-libs-windows-arm64.tar.gz", "159ce60eb587112b8c5f290b8beea56af5f13be4d13679de127ad9bfaafb7447", 18_389_170, "duckdb.dll"),
        _ => None,
    }
}

pub fn download_url(a: &Asset) -> String {
    format!("https://github.com/duckdb/duckdb/releases/download/v{DUCKDB_VERSION}/{}", a.file)
}

static RUNTIME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where downloaded runtimes live (the app sets its data folder at startup).
pub fn set_runtime_dir(dir: PathBuf) {
    let _ = RUNTIME_DIR.set(dir);
}

fn install_dir() -> Option<PathBuf> {
    RUNTIME_DIR.get().map(|d| d.join(format!("duckdb-{DUCKDB_VERSION}")))
}

/// The library to load: `DATABUG_DUCKDB_LIB`, else the downloaded copy.
pub fn library_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DATABUG_DUCKDB_LIB") {
        return Some(PathBuf::from(p));
    }
    Some(install_dir()?.join(asset()?.library))
}

/// Whether a DuckDB library is ready to load.
pub fn installed() -> bool {
    library_path().map(|p| p.is_file()).unwrap_or(false)
}

use crate::InstallState;

static STATE: Mutex<InstallState> = Mutex::new(InstallState { running: false, downloaded: 0, total: 0, error: None });

pub fn install_state() -> InstallState {
    STATE.lock().map(|s| s.clone()).unwrap_or_default()
}

fn update(f: impl FnOnce(&mut InstallState)) {
    if let Ok(mut s) = STATE.lock() {
        f(&mut s);
    }
}

/// Start downloading DuckDB in the background. `notify` is called whenever
/// the progress changes (the dialog uses it to repaint). Does nothing if a
/// download is already running.
pub fn start_install(rt: &tokio::runtime::Handle, notify: Arc<dyn Fn() + Send + Sync>) {
    {
        let Ok(mut s) = STATE.lock() else { return };
        if s.running {
            return;
        }
        *s = InstallState { running: true, ..Default::default() };
    }
    rt.spawn(async move {
        let res = install(&notify).await;
        update(|s| {
            s.running = false;
            s.error = res.err().map(|e| format!("{e:#}"));
        });
        notify();
    });
}

/// TLS that verifies the server against the public web roots (unlike the
/// database drivers, which accept self-signed certificates).
fn verifying_client() -> anyhow::Result<reqwest::Client> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth();
    Ok(reqwest::Client::builder().use_preconfigured_tls(tls).connect_timeout(std::time::Duration::from_secs(15)).user_agent(concat!("DataBug/", env!("CARGO_PKG_VERSION"))).build()?)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn install(notify: &Arc<dyn Fn() + Send + Sync>) -> anyhow::Result<PathBuf> {
    let a = asset().ok_or_else(|| anyhow::anyhow!("DuckDB doesn't publish a library for {} {}", std::env::consts::OS, std::env::consts::ARCH))?;
    let dir = install_dir().ok_or_else(|| anyhow::anyhow!("DataBug's data folder isn't set"))?;
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(format!("{}.part", a.file));
    let mut resp = crate::http::check(verifying_client()?.get(download_url(&a)).send().await?).await?;
    update(|s| s.total = resp.content_length().unwrap_or(a.size));
    let mut file = std::fs::File::create(&archive)?;
    let mut hash = Sha256::new();
    let mut n = 0u64;
    let mut last = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        use std::io::Write;
        file.write_all(&chunk)?;
        hash.update(&chunk);
        n += chunk.len() as u64;
        if n - last > 256 * 1024 {
            last = n;
            update(|s| s.downloaded = n);
            notify();
        }
    }
    drop(file);
    update(|s| s.downloaded = n);
    let got = hex(&hash.finalize());
    if got != a.sha256 {
        let _ = std::fs::remove_file(&archive);
        anyhow::bail!("The download didn't match DuckDB {DUCKDB_VERSION}'s published checksum (got {got}), so it was deleted");
    }
    let (ap, d, lib) = (archive.clone(), dir.clone(), a.library);
    let out = tokio::task::spawn_blocking(move || extract(&ap, &d, lib)).await?;
    let _ = std::fs::remove_file(&archive);
    if out.is_ok() {
        remove_older_versions(&dir);
    }
    out
}

/// Delete libraries left by earlier DuckDB versions (`runtimes/duckdb-<old>`).
fn remove_older_versions(current: &Path) {
    let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for e in entries.flatten() {
        let n = e.file_name();
        if n != name && n.to_string_lossy().starts_with("duckdb-") && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Unpack only the library from the .tar.gz, through a temporary name so a
/// half-written file is never loaded.
fn extract(archive: &Path, dir: &Path, library: &str) -> anyhow::Result<PathBuf> {
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(std::fs::File::open(archive)?));
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let is_lib = entry.header().entry_type().is_file() && entry.path()?.file_name().is_some_and(|n| n == library);
        if !is_lib {
            continue;
        }
        let tmp = dir.join(format!("{library}.part"));
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        let dest = dir.join(library);
        std::fs::rename(&tmp, &dest)?;
        return Ok(dest);
    }
    anyhow::bail!("{library} is missing from the download")
}

// ---------------------------------------------------------------------------
// The C API, resolved from the library at run time.

pub(crate) type Handle = *mut c_void;

/// `duckdb_result`: six pointer-sized fields; only DuckDB reads them.
#[repr(C)]
pub(crate) struct RawResult {
    _column_count: u64,
    _row_count: u64,
    _rows_changed: u64,
    _columns: *mut c_void,
    _error: *mut c_char,
    _internal: *mut c_void,
}

impl RawResult {
    pub fn zeroed() -> Self {
        RawResult { _column_count: 0, _row_count: 0, _rows_changed: 0, _columns: std::ptr::null_mut(), _error: std::ptr::null_mut(), _internal: std::ptr::null_mut() }
    }
}

macro_rules! api {
    ($($field:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub(crate) struct Api {
            _lib: libloading::Library,
            $(pub $field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            /// # Safety
            /// `path` must be a DuckDB library whose C API matches these
            /// signatures (DuckDB 1.x and 2.x); loading it runs its initialisers.
            unsafe fn load(path: &Path) -> anyhow::Result<Api> {
                // SAFETY: the caller guarantees `path` is a compatible DuckDB library.
                let lib = unsafe { libloading::Library::new(path)? };
                // SAFETY: each symbol is looked up by its DuckDB C API name and given
                // that function's documented signature; `_lib` keeps the library
                // loaded for as long as the pointers are used.
                $(let $field = unsafe { *lib.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(concat!("duckdb_", stringify!($field), "\0").as_bytes())
                    .map_err(|e| anyhow::anyhow!("{} isn't a usable DuckDB library: {e}", path.display()))? };)*
                Ok(Api { _lib: lib, $($field,)* })
            }
        }
    };
}

api! {
    open_ext: fn(*const c_char, *mut Handle, Handle, *mut *mut c_char) -> c_int;
    create_config: fn(*mut Handle) -> c_int;
    set_config: fn(Handle, *const c_char, *const c_char) -> c_int;
    destroy_config: fn(*mut Handle);
    connect: fn(Handle, *mut Handle) -> c_int;
    disconnect: fn(*mut Handle);
    close: fn(*mut Handle);
    query: fn(Handle, *const c_char, *mut RawResult) -> c_int;
    result_error: fn(*mut RawResult) -> *const c_char;
    column_count: fn(*mut RawResult) -> u64;
    column_name: fn(*mut RawResult, u64) -> *const c_char;
    row_count: fn(*mut RawResult) -> u64;
    rows_changed: fn(*mut RawResult) -> u64;
    value_is_null: fn(*mut RawResult, u64, u64) -> bool;
    value_varchar: fn(*mut RawResult, u64, u64) -> *mut c_char;
    destroy_result: fn(*mut RawResult);
    interrupt: fn(Handle);
    free: fn(*mut c_void);
}

static API: Mutex<Option<Arc<Api>>> = Mutex::new(None);

/// The loaded library, loading it on first use. A failed load isn't
/// remembered, so a library downloaded afterwards is picked up.
pub(crate) fn api() -> anyhow::Result<Arc<Api>> {
    let mut slot = API.lock().map_err(|_| anyhow::anyhow!("DuckDB loader poisoned"))?;
    if let Some(a) = slot.as_ref() {
        return Ok(a.clone());
    }
    let path = library_path().filter(|p| p.is_file()).ok_or_else(|| {
        let size = asset().map(|a| format!(" ({} MB)", a.size / 1_000_000)).unwrap_or_default();
        anyhow::anyhow!("DuckDB isn't installed yet. Open New connection, choose DuckDB and click Download DuckDB{size}.")
    })?;
    // SAFETY: loading the library runs its initialisers; it's DuckDB's
    // official build (checked by hash) or one the user pointed us at.
    let api = Arc::new(unsafe { Api::load(&path)? });
    *slot = Some(api.clone());
    Ok(api)
}

pub(crate) fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: DuckDB returns NUL-terminated strings
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_release_platform_has_an_asset() {
        for (os, arch) in [("macos", "aarch64"), ("macos", "x86_64"), ("linux", "x86_64"), ("linux", "aarch64"), ("windows", "x86_64"), ("windows", "aarch64")] {
            let a = asset_for(os, arch).unwrap_or_else(|| panic!("{os} {arch}"));
            assert_eq!(a.sha256.len(), 64);
            assert!(a.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(download_url(&a).starts_with("https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-shared-libs-"));
            assert!(a.file.ends_with(".tar.gz"));
        }
        assert!(asset_for("freebsd", "x86_64").is_none());
        assert_eq!(asset_for("windows", "x86_64").unwrap().library, "duckdb.dll");
    }

    #[test]
    fn hex_digest() {
        assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn extracts_only_the_library() {
        let dir = std::env::temp_dir().join(format!("databug-ducktar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ap = dir.join("lib.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&ap).unwrap(), flate2::Compression::fast());
        let mut w = tar::Builder::new(gz);
        for (name, body) in [("duckdb.h", &b"header"[..]), ("libduckdb.so", &b"library bytes"[..])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            w.append_data(&mut h, name, body).unwrap();
        }
        w.into_inner().unwrap().finish().unwrap();
        let out = extract(&ap, &dir, "libduckdb.so").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"library bytes");
        assert!(!dir.join("duckdb.h").exists());
        assert!(extract(&ap, &dir, "duckdb.dll").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_versions_are_removed() {
        let root = std::env::temp_dir().join(format!("databug-duckold-{}", std::process::id()));
        for d in ["duckdb-1.5.6", "duckdb-2.0.0", "other"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        remove_older_versions(&root.join("duckdb-2.0.0"));
        assert!(!root.join("duckdb-1.5.6").exists());
        assert!(root.join("duckdb-2.0.0").exists());
        assert!(root.join("other").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
