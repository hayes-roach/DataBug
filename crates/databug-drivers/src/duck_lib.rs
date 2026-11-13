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
pub const DUCKDB_VERSION: &str = "1.5.6";

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
    // From https://github.com/duckdb/duckdb/releases/tag/v1.5.6 (GitHub's published digests)
    let a = |file, sha256, size, library| Some(Asset { file, sha256, size, library });
    match (os, arch) {
        ("macos", _) => a("libduckdb-osx-universal.zip", "e0bc007d9b0094c0970ac1847a8601d10aad07cbd2910ce586ec810f77b638d6", 36_216_941, "libduckdb.dylib"),
        ("linux", "x86_64") => a("libduckdb-linux-amd64.zip", "b845005f5132a7d8180057c35e14a7626632258782f871a90861b19c1c03841b", 41_378_782, "libduckdb.so"),
        ("linux", "aarch64") => a("libduckdb-linux-arm64.zip", "b72ed9f05003f5e9d2015f7ceada6416b377d9dd33169cdde3c9e33897856eee", 37_763_835, "libduckdb.so"),
        ("windows", "x86_64") => a("libduckdb-windows-amd64.zip", "44cf59583f9951d2cb09b1bf115a63ecb2d8901e363903029d86c7d8683fe96a", 13_408_787, "duckdb.dll"),
        ("windows", "aarch64") => a("libduckdb-windows-arm64.zip", "c63285647e94c46d309b60d5dd1ff3cb2cdf9ca03e415a818ff7d8dbf838e0e8", 14_366_908, "duckdb.dll"),
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
    let zip_path = dir.join(format!("{}.part", a.file));
    let mut resp = crate::http::check(verifying_client()?.get(download_url(&a)).send().await?).await?;
    update(|s| s.total = resp.content_length().unwrap_or(a.size));
    let mut file = std::fs::File::create(&zip_path)?;
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
        let _ = std::fs::remove_file(&zip_path);
        anyhow::bail!("The download didn't match DuckDB {DUCKDB_VERSION}'s published checksum (got {got}), so it was deleted");
    }
    let (zp, d, lib) = (zip_path.clone(), dir.clone(), a.library);
    let out = tokio::task::spawn_blocking(move || extract(&zp, &d, lib)).await?;
    let _ = std::fs::remove_file(&zip_path);
    out
}

/// Unpack only the library, through a temporary name so a half-written
/// file is never loaded.
fn extract(zip_path: &Path, dir: &Path, library: &str) -> anyhow::Result<PathBuf> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut entry = z.by_name(library).map_err(|_| anyhow::anyhow!("{library} is missing from the download"))?;
    let tmp = dir.join(format!("{library}.part"));
    let mut out = std::fs::File::create(&tmp)?;
    std::io::copy(&mut entry, &mut out)?;
    drop(out);
    let dest = dir.join(library);
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
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
            /// signatures (DuckDB 1.x); loading it runs its initialisers.
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
            assert!(download_url(&a).starts_with("https://github.com/duckdb/duckdb/releases/download/v1.5.6/libduckdb-"));
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
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("databug-duckzip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zp = dir.join("lib.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("duckdb.h", opts).unwrap();
        w.write_all(b"header").unwrap();
        w.start_file("libduckdb.so", opts).unwrap();
        w.write_all(b"library bytes").unwrap();
        w.finish().unwrap();
        let out = extract(&zp, &dir, "libduckdb.so").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"library bytes");
        assert!(!dir.join("duckdb.h").exists());
        assert!(extract(&zp, &dir, "duckdb.dll").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
