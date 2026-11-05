//! Checking GitHub for a newer release and installing it.
//!
//! The check asks the GitHub API for the latest release (no account, no
//! data about you beyond the request itself) and compares its tag with this
//! build's version. Installing downloads the release asset for this
//! platform, verifies it against the release's `SHA256SUMS.txt`, and swaps
//! it in: macOS replaces the running `DataBug.app`, Windows runs the setup
//! silently, Linux reruns the bundled `install.sh`. DataBug then restarts.
//! Nothing is installed unless you click Update.
//!
//! Downloads use the system `curl` (part of macOS, Windows 10+ and most
//! Linux distributions), so no TLS stack is compiled into DataBug for this.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPO: &str = "hayes-roach/DataBug";

/// This build's version.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone)]
pub struct Release {
    /// Tag as published, e.g. `v0.2.0`.
    pub tag: String,
    pub version: (u64, u64, u64),
    pub notes: String,
    pub page: String,
    /// (file name, download URL)
    pub assets: Vec<(String, String)>,
}

impl Release {
    pub fn version_string(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }

    fn asset(&self, name: &str) -> Option<&str> {
        self.assets.iter().find(|(n, _)| n == name).map(|(_, u)| u.as_str())
    }
}

/// `v1.2.3`, `1.2`, `1.2.3-beta` → (1, 2, 3); anything else → None.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

/// Where the release information comes from. `DATABUG_UPDATE_URL` points
/// somewhere else (a mirror, or a local file for testing).
fn latest_url() -> String {
    std::env::var("DATABUG_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"))
}

fn curl() -> Command {
    let mut c = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    c.args(["--fail", "--silent", "--show-error", "--location", "--max-time", "600", "--retry", "2"]);
    c.arg("--user-agent").arg(format!("DataBug/{}", current_version()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn fetch_text(url: &str) -> Result<String, String> {
    let out = curl().arg("--header").arg("Accept: application/vnd.github+json").arg(url).output().map_err(|e| format!("Couldn't run curl to check for updates: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Couldn't reach GitHub: {}", err.trim().lines().last().unwrap_or("request failed")));
    }
    String::from_utf8(out.stdout).map_err(|_| "GitHub sent an unreadable answer".to_string())
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let out = curl().arg("--output").arg(to).arg(url).output().map_err(|e| format!("Couldn't run curl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Download failed: {}", String::from_utf8_lossy(&out.stderr).trim().lines().last().unwrap_or("request failed")))
    }
}

/// Parse the GitHub "latest release" JSON.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Unexpected answer from GitHub: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("The latest release has no tag")?.to_string();
    let version = parse_version(&tag).ok_or_else(|| format!("Can't read the release version “{tag}”"))?;
    let assets = v["assets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Ok(Release {
        tag,
        version,
        notes: v["body"].as_str().unwrap_or("").to_string(),
        page: v["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        assets,
    })
}

/// The latest release if it's newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let r = parse_release(&fetch_text(&latest_url())?)?;
    let current = parse_version(current_version()).unwrap_or((0, 0, 0));
    Ok((r.version > current).then_some(r))
}

/// How this copy of DataBug was installed, which decides how it updates.
#[derive(Debug, Clone, PartialEq)]
pub enum InstallKind {
    /// `…/DataBug.app` on macOS
    MacApp(PathBuf),
    /// Installed by the Windows setup; `true` when installed for all users.
    WindowsSetup(bool),
    /// `~/.local/bin/databug` from the Linux install script
    LinuxLocal(PathBuf),
    /// Built from source or run from somewhere else: update by hand.
    Manual(String),
}

pub fn install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize().or(Ok(p))) else { return InstallKind::Manual("Can't tell where DataBug is running from".into()) };
    if exe.components().any(|c| c.as_os_str() == "target") {
        return InstallKind::Manual("This is a build from source. Update it with `git pull` and `cargo build --release`.".into());
    }
    if cfg!(target_os = "macos") {
        if let Some(app) = exe.ancestors().nth(3).filter(|p| p.extension().map(|e| e == "app").unwrap_or(false)) {
            return InstallKind::MacApp(app.to_path_buf());
        }
        return InstallKind::Manual("DataBug isn't running from DataBug.app, so it can't replace itself.".into());
    }
    if cfg!(windows) {
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if dir.join("unins000.exe").exists() {
            let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
            let per_user = local.map(|l| dir.starts_with(l)).unwrap_or(false);
            return InstallKind::WindowsSetup(!per_user);
        }
        return InstallKind::Manual("This copy wasn't installed with the DataBug setup (portable zip), so update it by downloading the new version.".into());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if exe == home.join(".local/bin/databug") {
        return InstallKind::LinuxLocal(exe);
    }
    InstallKind::Manual("DataBug wasn't installed with install.sh, so update it the way you installed it.".into())
}

fn asset_name() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("DataBug-macos-universal.zip")
    } else if cfg!(windows) {
        Some("DataBug-windows-x86_64-setup.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("DataBug-linux-x86_64.tar.gz")
    } else {
        None
    }
}

/// SHA-256 of a file, lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Find `name`'s checksum in a `sha256sum` style list.
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let sum = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file == name).then(|| sum.to_ascii_lowercase())
    })
}

/// What happened after an install step, for the UI.
pub enum Installed {
    /// The new version is in place (or the installer is running); quit so
    /// the helper can finish and start the new version.
    Restart,
}

/// Download, verify and install `r`. Runs for a while; call it off the UI thread.
pub fn install(r: &Release, progress: &dyn Fn(&str)) -> Result<Installed, String> {
    let kind = install_kind();
    if let InstallKind::Manual(why) = &kind {
        return Err(why.clone());
    }
    let name = asset_name().ok_or("There's no download for this platform yet")?;
    let url = r.asset(name).ok_or_else(|| format!("The release has no {name}"))?;
    let dir = std::env::temp_dir().join(format!("databug-update-{}", r.version_string()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let file = dir.join(name);

    progress(&format!("Downloading DataBug {}…", r.version_string()));
    download(url, &file)?;

    progress("Verifying the download…");
    let sums_url = r.asset("SHA256SUMS.txt").ok_or("The release has no SHA256SUMS.txt, so the download can't be verified")?;
    let sums_file = dir.join("SHA256SUMS.txt");
    download(sums_url, &sums_file)?;
    let sums = std::fs::read_to_string(&sums_file).map_err(|e| e.to_string())?;
    let want = expected_sum(&sums, name).ok_or_else(|| format!("SHA256SUMS.txt doesn't list {name}"))?;
    let got = sha256_file(&file)?;
    if got != want {
        return Err(format!("The download is damaged or was changed (checksum {got} instead of {want}). Nothing was installed."));
    }

    progress("Installing…");
    match kind {
        InstallKind::MacApp(app) => install_mac(&app, &file, &dir),
        InstallKind::WindowsSetup(all_users) => install_windows(&file, all_users),
        InstallKind::LinuxLocal(exe) => install_linux(&exe, &file, &dir),
        InstallKind::Manual(why) => Err(why),
    }
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn relaunch_wanted() -> bool {
    std::env::var_os("DATABUG_UPDATE_NO_RELAUNCH").is_none()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn install_mac(app: &Path, zip: &Path, dir: &Path) -> Result<Installed, String> {
    let parent = app.parent().ok_or("Can't find the folder DataBug.app is in")?;
    let probe = parent.join(".databug-update-check");
    std::fs::write(&probe, b"").map_err(|_| format!("DataBug can't write to {}. Move DataBug.app to a folder you can change (like Applications), or update from the release page.", parent.display()))?;
    let _ = std::fs::remove_file(&probe);
    let unpacked = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    run(Command::new("ditto").arg("-x").arg("-k").arg(zip).arg(&unpacked), "Couldn't unpack the update")?;
    let new_app = find_named(&unpacked, "DataBug.app", 3).ok_or("The download doesn't contain DataBug.app")?;
    // A helper waits for DataBug to quit, swaps the app (keeping the old one
    // until the new one is in place) and opens the new version.
    let script = dir.join("swap.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
set -u
PID="$1"; APP="$2"; NEW="$3"; OPEN="$4"
case "$APP" in *.app) ;; *) exit 1 ;; esac
while kill -0 "$PID" 2>/dev/null; do sleep 0.3; done
OLD="$APP.previous"
rm -rf "$OLD"
if mv "$APP" "$OLD" && ditto "$NEW" "$APP"; then
  rm -rf "$OLD"
else
  rm -rf "$APP"; mv "$OLD" "$APP"
fi
xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true
[ "$OPEN" = 1 ] && open "$APP"
exit 0
"#,
    )
    .map_err(|e| e.to_string())?;
    Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(app)
        .arg(&new_app)
        .arg(if relaunch_wanted() { "1" } else { "0" })
        .spawn()
        .map_err(|e| format!("Couldn't start the updater: {e}"))?;
    Ok(Installed::Restart)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn install_windows(setup: &Path, all_users: bool) -> Result<Installed, String> {
    // The setup closes DataBug if it's still open, installs over the same
    // folder, and starts the new version (`/relaunch=1`).
    let mut c = Command::new(setup);
    c.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]);
    c.arg(if all_users { "/ALLUSERS" } else { "/CURRENTUSER" });
    if relaunch_wanted() {
        c.arg("/relaunch=1");
    }
    c.spawn().map_err(|e| format!("Couldn't start the installer: {e}"))?;
    Ok(Installed::Restart)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn install_linux(exe: &Path, tarball: &Path, dir: &Path) -> Result<Installed, String> {
    let unpacked = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    run(Command::new("tar").arg("xzf").arg(tarball).arg("-C").arg(&unpacked), "Couldn't unpack the update")?;
    let script = unpacked.join("DataBug").join("install.sh");
    if !script.exists() {
        return Err("The download doesn't contain install.sh".into());
    }
    // install.sh replaces the file rather than writing into it, so the
    // running copy keeps working until it quits
    run(Command::new("sh").arg(&script), "The install script failed")?;
    if relaunch_wanted() {
        Command::new("/bin/sh")
            .arg("-c")
            .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.3; done; exec \"$2\"")
            .arg("databug-relaunch")
            .arg(std::process::id().to_string())
            .arg(exe)
            .spawn()
            .map_err(|e| format!("Couldn't restart DataBug: {e}"))?;
    }
    Ok(Installed::Restart)
}

fn find_named(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        return Some(p);
    }
    if depth == 0 {
        return None;
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.path().is_dir() {
            if let Some(f) = find_named(&e.path(), name, depth - 1) {
                return Some(f);
            }
        }
    }
    None
}

/// Open the release page in the browser.
pub fn open_page(url: &str) {
    let _ = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        Command::new("explorer").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("v2.0.1-beta.1"), Some((2, 0, 1)));
        assert_eq!(parse_version("latest"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    #[test]
    fn release_json() {
        let r = parse_release(
            r#"{"tag_name":"v0.3.1","body":"Fixes","html_url":"https://github.com/x/y/releases/tag/v0.3.1",
               "assets":[{"name":"SHA256SUMS.txt","browser_download_url":"https://example.com/s"},
                         {"name":"DataBug-macos-universal.zip","browser_download_url":"https://example.com/m"}]}"#,
        )
        .unwrap();
        assert_eq!(r.version, (0, 3, 1));
        assert_eq!(r.asset("DataBug-macos-universal.zip"), Some("https://example.com/m"));
        assert!(r.asset("nope").is_none());
    }

    #[test]
    fn checksums() {
        let sums = "abc123  DataBug-linux-x86_64.tar.gz\nDEF456 *DataBug-windows-x86_64-setup.exe\n";
        assert_eq!(expected_sum(sums, "DataBug-linux-x86_64.tar.gz").as_deref(), Some("abc123"));
        assert_eq!(expected_sum(sums, "DataBug-windows-x86_64-setup.exe").as_deref(), Some("def456"));
        assert!(expected_sum(sums, "other").is_none());
        let p = std::env::temp_dir().join(format!("databug-sha-test-{}", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(&p);
    }
}
