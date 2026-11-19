# Installing and updating DataBug

Back to the [README](../README.md).

| Platform | Installer | One-line install |
|---|---|---|
| **macOS** (Apple silicon and Intel) | `DataBug-macos-universal.dmg`: open it and drag DataBug to Applications | `curl -fsSL https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.sh \| sh` |
| **Windows** 10/11 (x64) | `DataBug-windows-x86_64-setup.exe`: installs for your user (no admin needed), adds Start menu and optional desktop shortcuts, uninstall from Settings → Apps | `irm https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.ps1 \| iex` (PowerShell) |
| **Linux** (x86_64) | `DataBug-linux-x86_64.tar.gz`: extract and run `DataBug/install.sh` (app, icon and menu entry in `~/.local`) | `curl -fsSL https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.sh \| sh` |

Download the installers from the [latest release](https://github.com/hayes-roach/DataBug/releases/latest). A first install lets you choose the folder: the Windows setup shows a *Select Destination Location* page, and the one-line installs ask (press Enter for the default; set `DATABUG_INSTALL_DIR` to choose without being asked, or pass `--yes` / set `DATABUG_YES=1` to take the default). Updates go to the same folder, and an update leaves only the new version: copies of DataBug in other folders (another Applications folder, an install for all users next to one for you), backups from an interrupted update and earlier update downloads are removed. Running a one-line install again updates an existing install in place (it says "Updating 0.1.0 → 0.2.0", or "already up to date" and stops; to reinstall anyway, add `-s -- --force` after `sh` on macOS/Linux, or run `$env:DATABUG_FORCE = "1"` first in PowerShell). Both close a running DataBug first, verify the download against the release's checksums, and on Windows update an existing install where it is (asking for admin rights only if it was installed for all users). If the Windows installer still fails, the script says why and shows the relevant lines of the setup log. To uninstall: drag DataBug.app to the Trash on macOS, use Settings → Apps on Windows, or run `install.sh --uninstall` (either script) on macOS/Linux. Settings and history are kept.

The apps aren't signed with a paid Apple or Microsoft certificate yet. macOS builds are signed with DataBug's own certificate, so every version has the same identity: when macOS asks whether DataBug may use a password saved in your keychain, choose **Always Allow** once and updates won't ask again (passwords DataBug saves itself never ask). On macOS, if Gatekeeper blocks the first launch of the DMG install, right-click DataBug in Applications → *Open* (the one-line installer avoids this). On Windows, SmartScreen may show "Windows protected your PC": click *More info* → *Run anyway*.

## Updates

DataBug checks GitHub for a newer release when it starts (at most every 6 hours) and once a day while it's open. When one is out, an **Update to X.Y.Z** button appears in the toolbar. Nothing changes until you click it:

- **Update and restart** downloads the new version for your platform, checks it against the release's `SHA256SUMS.txt` (a damaged or altered download is refused and nothing is installed), installs it and reopens DataBug. Connections, settings, passwords and history are kept.
  - macOS: replaces `DataBug.app` where it is (it needs to be in a folder you can write to, like Applications)
  - Windows: runs the new setup silently over the existing install (per-user or all-users, whichever you have)
  - Linux: reruns the new release's `install.sh` into `~/.local`
- **Skip this version** stops reminders for that release; **Later** hides the window until next time.
- Copies DataBug can't replace itself (a build from source, the portable Windows zip, or a Linux binary not installed with `install.sh`) still get the notice, with a link to the release or, for source builds, `git pull && cargo build --release`.
- *Settings (⚙) → Check for updates automatically* turns the checks off; *Check for updates* checks right away. The check is a single request to GitHub's public releases API and sends nothing about you or your databases. Downloads use the system `curl`.

## Making a release

Releases are made only on request: run *Cut release* in the Actions tab (`gh workflow run auto-release.yml`), which bumps the version and changelog and pushes a `vX.Y.Z` tag, or push a tag yourself (see [CONTRIBUTING.md](../CONTRIBUTING.md)). The tag starts `.github/workflows/release.yml`, which builds the DMG, setup .exe, portable Windows zip and Linux tarball, adds `SHA256SUMS.txt`, and publishes a GitHub release with install instructions. Every push to `main` also uploads the macOS and Windows installers as CI artifacts.

To build installers yourself:
- macOS: `packaging/macos/bundle.sh` (this Mac's architecture) or `packaging/macos/bundle.sh universal` → `dist/DataBug-macos-*.dmg` and `.zip`
- Windows: `cargo build --release`, then `iscc packaging\windows\databug.iss` with [Inno Setup 6](https://jrsoftware.org/isinfo.php) → `dist\DataBug-windows-x86_64-setup.exe`
- Linux: `cargo build --release`, then copy `target/release/databug` next to `packaging/linux/install.sh` and run it

The in-app updater reads the latest GitHub release, so every release must include the platform assets under their usual names and `SHA256SUMS.txt` (the release workflow does this).
