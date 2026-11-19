# Changelog

All notable changes to DataBug are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** in the same pull request as the change, in the section that fits: Added, Changed, Deprecated, Removed, Fixed or Security.

## [Unreleased]

## [0.8.2] - 2026-11-19

### Fixed

- DuckDB files couldn't be opened on Windows ("The process cannot access the file because it is being used by another process"): each DataBug session opened its own copy of the database, and Windows refuses a second lock on the file even from the same app. Sessions on the same file now share one open database, each with its own connection, and the file is released when the last one closes

## [0.8.1] - 2026-11-19

### Changed

- DuckDB is now 2.0.0 (was 1.5.6), so DataBug opens database files written by DuckDB 2.0. Connecting asks you to download the new library once (about 40 MB on macOS), and the old one is deleted afterwards. Files DataBug changes are then in DuckDB 2.0's format, which DuckDB 1.x can't open

## [0.8.0] - 2026-11-19

### Added

- Rename a connection in place from the explorer: right-click → *Rename…*, or `F2` over its row

### Changed

- The connection button only opens its menu while connected. Clicking it while signed out connects once with the saved credentials and opens the menu when that works
- Explorer connection rows no longer spell out the engine and state ("sql server · connected"): the logo shows the engine and the dot the state. The tooltip still has both
- Closing DataBug disconnects from every database first

## [0.7.0] - 2026-11-18

### Added

- Putting the cursor on a name softly highlights its other uses in the console (by token, so matching text in strings, comments and keywords isn't highlighted)

### Changed

- New layout for the toolbar. The top bar holds the connection button, which shows the console's database and schema and opens a menu with the database and schema pickers and the connection's consoles, plus settings and theme. Run, Explain, Cancel, the commit mode (*Auto-commit* / *Manual commit*), Commit, Rollback and Format sit in a bar above the editor. Every control is the same height with the same corners and border, Run is the one filled button, *PRODUCTION*, *Read-only* and *Transaction open* are pills, and the icons are drawn instead of emoji so they look the same on every platform
- **Run** runs the selected code, or the whole console when nothing is selected (the separate *Run script* button is gone). `Ctrl/⌘ + Enter` still runs just the statement at the cursor; `Ctrl/⌘ + Shift + Enter` does what Run does
- On SQL Server, Run sends each batch between `GO` lines as one request, as SQL Server Management Studio does, instead of one statement at a time, so a script with `DECLARE`, temp tables or `SET` options runs as a whole

### Fixed

- A script starting with an invisible byte-order mark (common in files saved on Windows) hid its leading `DECLARE`, so SQL Server ran it one statement at a time
- Oracle and Db2 blocks were run one `;` at a time: `DECLARE … BEGIN … END;` and `BEGIN … END;` blocks, procedures, functions and triggers now run whole, and packages up to a `/` line. A PL/SQL `BEGIN … END` block (or T-SQL `BEGIN TRY`) is no longer mistaken for the start of a transaction
- Selecting a single letter (or a few) by dragging in the editor works; before, a drag shorter than about 6 px counted as a click and selected nothing

## [0.6.1] - 2026-11-18

### Changed

- Releases are cut only on request (the *Cut release* workflow, run by hand); pushes to `main` no longer release on their own, and the release commit and tag are pushed together so a release can't be left half-made

### Fixed

- Right-clicking a selection in the editor no longer clears it, so *Cut* and *Copy* in the menu act on what you selected
- The connection dialog's checkboxes (*Save in system keychain*, *Trust server certificate*, *Production*, *Read-only*, schema list) match the rest of the dialog and sit apart from the fields next to them
- More room around the *Port* label in the connection dialog

## [0.6.0] - 2026-11-18

### Added

- Right-click menu in the editor: Cut, Copy, Paste, Select all, Run statement or selection, Run script, Go to definition, Find, Format and Toggle comment
- Output messages can be selected and copied, and right-clicking one copies the message, its SQL, the line or the whole output

### Changed

- The consoles list now holds only consoles you haven't closed: the ones open now and the ones still open when DataBug last quit. It moved from the welcome screen and the tab bar to a consoles button in the toolbar (next to the schema picker), shown while the connection is signed in. Closing a tab removes its console; `Ctrl/⌘ + Shift + T` still brings back the last one closed during this run
- Every text field (table editor, CSV import, rename, find and replace, Find in database, result and history filters, schema and database pickers) uses the same style as the new-connection dialog instead of a black box
- README: dropped the comparisons to DataGrip

### Fixed

- Dragging a selection in the editor past its top or bottom edge now keeps scrolling, so the cursor follows the pointer instead of stopping at the edge
- Schema compare's difference count ("6 differences") no longer splits across two lines when the toolbar is narrow
- README screenshots updated for the new connection dialog, logos, explorer refresh button, consoles button and text fields, with new ones for the engine list, the consoles list and go to definition

## [0.5.0] - 2026-11-18

### Changed

- Oracle, IBM Db2 and Spark SQL are now in every download instead of being opt-in build features, so they no longer show "not in this build". The ODBC driver manager is built in (unixODBC compiled in on macOS and Linux, Windows' own ODBC), so DataBug still starts without any vendor software; connecting needs Oracle Instant Client or the engine's ODBC driver, as before
- Every engine now shows its real logo: Oracle, IBM Db2, DuckDB, Cassandra, CockroachDB, TimescaleDB, QuestDB, InfluxDB, Spark and H2 replace DataBug's two-letter tiles (sources and licenses in `assets/logos/SOURCES.md`). The Windows installer removes the old tiles once so the new logos show up after an update

## [0.4.0] - 2026-11-18

### Added

- Go to definition: `Ctrl/⌘ + B`, `Ctrl/⌘ + click`, or `Control + F` on macOS on the name of a procedure, function, trigger or view opens its source in a new console (a table opens its DDL)
- A *Sponsor* button on the GitHub repository and a Sponsor section in the README, for anyone who wants to support DataBug
- A refresh button in the explorer header (left of Find) reloads the current database: the open console's, or the one selected in the explorer
- Right-click a tab to close it, the other tabs, the tabs to its left or right, or all tabs
- Closed consoles are kept (with their connection and schema, across restarts) and listed behind the clock button next to the tabs and on the welcome screen; click to reopen, × to delete. `Ctrl/⌘ + Shift + T` reopens the last one closed. Empty consoles aren't kept
- Each console has its own schema (and database, when a connection shows several), shown and switched from the toolbar. It drives completion, inspections and go to definition, and the session's schema on engines that have one (`search_path`, `USE`, `CURRENT_SCHEMA`, `SET SCHEMA`)

### Changed

- The new-connection dialog picks the database type from a dropdown with each engine's logo and name, grouped by kind, instead of a grid of tiles, and its text fields are the same size and style as that dropdown, with their labels lined up
- On Linux, DataBug draws its own title bar: minimize, maximize and close (an ×) match the app, the toolbar drags the window and double-clicking it maximizes, and the edges resize it

### Fixed

- SQL Server: a script that declares variables (`DECLARE` / `SET` / `EXEC … @x`) ran one statement at a time, so later statements lost the variables. A batch with `DECLARE`, or one that creates or alters a procedure, function, trigger or view, now runs as one request

## [0.3.7] - 2026-11-18

### Changed

- Bump mongodb from 3.10.0 to 3.10.1 in the compatible group (#16)

## [0.3.6] - 2026-11-13

### Changed

- Record why the rand 0.7 advisory doesn't apply

## [0.3.5] - 2026-11-13

### Fixed

- The Windows one-line installer failed with "exit code 5" when DataBug was open or had been installed for all users. It now closes DataBug first (and lets setup close it), updates an existing install where it is, says when you're already up to date, verifies the download's checksum, times out instead of hanging on a stalled connection, and explains failures with the relevant lines of the setup log

## [0.3.4] - 2026-11-13

### Changed

- Snappier UI, especially on Windows: frames are shown as soon as they're drawn instead of queueing behind vsync (which made the hover highlight trail the mouse with high-polling mice and touchpads), and scrolling is immediate instead of animated
- The dot on a connection's icon is solid green while connected and a grey ring when disconnected

## [0.3.3] - 2026-11-13

### Security

- Upgraded the SQL Server driver (tiberius 0.13), whose TLS stack (rustls 0.21) accepted distrusted intermediate certificates and could be downgraded, and the bundled SQLite (rusqlite 0.42, SQLite with the fix for RUSTSEC-2026-0459). Also upgraded mysql_async (0.37, drops an unsound LRU cache) and zip (9)
- Passwords inside SQL (`PASSWORD '…'`, `IDENTIFIED BY …`, `SECRET = '…'`, MongoDB `pwd: …`) are masked before statements are saved to the query history
- CI now runs `cargo audit` and clippy with warnings as errors

### Fixed

- Editing rows on SQL Server turned characters outside the database's code page (é, 李, emoji) into `?`; string literals for SQL Server now use `N'…'`
- A result with more than 4 GB of text could crash DataBug; it now stops storing rows at that size and shows "result too large". Release builds also check integer overflow
- Failed saves of connections, settings, projects, history or the schema cache (full disk, permissions) were ignored; DataBug now shows a message
- After disconnecting, a connection could still show green dots; a connection the server had dropped could also still look connected

### Changed

- Connection dots follow the real connection state: the status dot is green only while signed in (amber while signing in), and the connection-color dots on the icon, toolbar and tabs turn into hollow rings when you're disconnected

## [0.3.2] - 2026-11-13

### Changed

- Double-clicking a table or view in the explorer (and running a procedure without parameters) shows only the results; the query is no longer added to the console
- Redesigned *Filter objects* field: search icon, clear button, Esc clears it
- *New project* / *Rename project* open a card with the name field focused and *Cancel* next to *Create*; the field no longer grabs focus back after you click elsewhere

## [0.3.1] - 2026-11-13

### Fixed

- The explorer no longer slows the whole window down on databases with thousands of tables, views or routines: it lays out only the rows on screen (about 0.1 ms per frame with 20,000 tables, down from 22 ms)

## [0.3.0] - 2026-11-05

### Added

- New engines: CockroachDB, TimescaleDB, QuestDB and H2 (PostgreSQL protocol, each with its own introspection), DuckDB (embedded), Apache Cassandra (CQL), CouchDB (Mango queries in shell syntax) and InfluxDB (InfluxQL, Flux and line protocol), all in the default build
- Opt-in drivers for Oracle (`oracle` feature, needs Oracle Instant Client), IBM Db2 (`db2`) and Spark SQL through its Thrift server (`spark`), both over ODBC; not yet tested against a live server
- The new-connection dialog groups engines by type (relational, analytical, document, wide-column, time series, distributed) and has the fields each engine needs: Oracle service name or SID, DuckDB file, Cassandra keyspace, InfluxDB organization, bucket and token, ODBC driver and options, H2 database path
- Monogram icons for the new engines, with the source and license of every logo in `assets/logos/SOURCES.md`
- Legal notices: a NOTICE file and a Disclaimer section in the README (use at your own risk, no warranty, not responsible for anything that happens from using DataBug, no affiliation with JetBrains or database vendors), a short notice in the app and installers, and LICENSE/NOTICE shipped with every download

### Changed

- DuckDB is no longer built into DataBug. Choosing DuckDB in the new-connection dialog offers *Download DuckDB*, which fetches DuckDB's official library (1.5.6) from its GitHub releases, checks it against a pinned SHA-256 and loads it when you connect; `DATABUG_DUCKDB_LIB` uses a library you already have. The download is about 13–41 MB depending on the platform, and DataBug itself is much smaller (27 MB instead of 72 MB on macOS)
- TLS for Cassandra moved to the opt-in `cassandra-tls` feature, so the default build needs no C crypto library; without it, choosing *Require* for Cassandra gives an error instead of connecting
- The macOS/Linux one-line installer now updates an existing install in place and says so ("Updating 0.1.0 → 0.2.0"), or reports that you're already up to date without downloading anything; `--force` reinstalls
- The welcome screen no longer offers "Open console on …" for the first saved connection

### Fixed

- The one-line installer on macOS could fail to reopen DataBug with `_LSOpenURLsWithCompletionHandler() failed with error -609` when DataBug was open during the install. It now waits for DataBug to close, registers the new copy with macOS, and retries the launch
- Postgres: when a result stopped at the row limit, the cancel request for the rest of it could cancel the next statement instead

## [0.2.0] - 2026-11-04

### Added

- In-app updates: DataBug checks GitHub for new releases and offers *Update and restart*, which downloads the new version, verifies its checksum, installs it and reopens; automatic checks can be turned off in settings. Coming from 0.1.0, install this release manually once; later updates install from inside the app
- Row editing in the results grid: select whole rows, delete them, add new rows or duplicate one, and save everything with Submit
- Contributing guide, code of conduct, security policy, issue and pull request templates

### Changed

- Double-clicking a table in the explorer opens its rows without expanding it
- Redesigned explorer header: icon buttons for Find and New connection, and a project switcher
- Building from source needs Rust 1.89 or newer (the dependencies already required it; 1.81 was listed before)
- README screenshots updated, with new ones for multiple databases, find and replace, and themes
- SQL Server marked as tested against a live server

### Fixed

- A failed sign-in was retried automatically, which could lock an account. DataBug now signs in once, shows the error, and only tries again when you ask (Retry, Connect, Refresh schema, running a query or editing the connection)

## [0.1.0] - 2026-11-04

First public release.

### Added

- Drivers for PostgreSQL, MySQL / MariaDB, SQL Server, SQLite and MongoDB, with schema introspection, streamed results, query plans and server-side cancel
- Schema explorer with tables, views, materialized views, procedures, functions, triggers, sequences, synonyms, events and SQL Server CLR objects, and a per-connection schema picker
- Several databases per PostgreSQL, SQL Server and MongoDB connection, each browsable as its own node
- Schema-aware SQL editor with completion, inspections, formatting and rename refactoring
- VS Code-style find and replace in the editor (match case, whole word, regex), and *Find in database* across object names, columns, definitions and table data
- Running procedures and functions from the explorer (`EXEC`, `CALL`, `SELECT` templates), and completion of procedures after `EXEC` / `CALL`
- Virtualized results grid with sorting, filtering, in-place editing and export
- Table editor, schema compare with migration DDL, ER diagrams and CSV import
- Projects with their own ordered connection lists; drag connections to reorder; disconnect and reconnect while keeping a connection listed
- Six built-in themes (Dark, Light, Midnight, Forest, Paper, High contrast) with per-theme color customization
- Production and read-only flags, confirmation for dangerous statements, manual transactions
- Installers: macOS DMG, Windows setup, Linux install script, and one-line installers

### Changed

- PostgreSQL connections with no database set now use the default `postgres` database

### Removed

- The built-in sample database from the welcome screen and explorer

### Fixed

- Editor text appeared grey in the light theme

[Unreleased]: https://github.com/hayes-roach/DataBug/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/hayes-roach/DataBug/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/hayes-roach/DataBug/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hayes-roach/DataBug/releases/tag/v0.1.0
