# Changelog

All notable changes to DataBug are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** in the same pull request as the change, in the section that fits: Added, Changed, Deprecated, Removed, Fixed or Security.

## [Unreleased]

### Changed

- The macOS/Linux one-line installer now updates an existing install in place and says so ("Updating 0.1.0 → 0.2.0"), or reports that you're already up to date without downloading anything; `--force` reinstalls
- The welcome screen no longer offers "Open console on …" for the first saved connection

### Fixed

- The one-line installer on macOS could fail to reopen DataBug with `_LSOpenURLsWithCompletionHandler() failed with error -609` when DataBug was open during the install. It now waits for DataBug to close, registers the new copy with macOS, and retries the launch

### Added

- Legal notices: a NOTICE file and a Disclaimer section in the README (use at your own risk, no warranty, not responsible for anything that happens from using DataBug, no affiliation with JetBrains or database vendors), a short notice in the app and installers, and LICENSE/NOTICE shipped with every download

## [0.2.0] - 2026-11-04

### Added

- In-app updates: DataBug checks GitHub for new releases and offers *Update and restart*, which downloads the new version, verifies its checksum, installs it and reopens; automatic checks can be turned off in settings. Coming from 0.1.0, install this release manually once; later updates install from inside the app
- Row editing in the results grid: select whole rows, delete them, add new rows or duplicate one, and save everything with Submit
- Contributing guide, code of conduct, security policy, issue and pull request templates
- New engines: CockroachDB, TimescaleDB, QuestDB and H2 (PostgreSQL protocol, each with its own introspection), Apache Cassandra (CQL; TLS with the `cassandra-tls` feature), CouchDB (Mango queries in shell syntax) and InfluxDB (InfluxQL, Flux and line protocol), all in the default build
- DuckDB, downloaded on demand: the connection dialog offers to fetch DuckDB's official library (pinned version and SHA-256) instead of building it into DataBug; `DATABUG_DUCKDB_LIB` uses a library you already have
- Opt-in drivers for Oracle (`oracle` feature, needs Oracle Instant Client), IBM Db2 (`db2`) and Spark SQL through its Thrift server (`spark`), both over ODBC; not yet tested against a live server
- The new-connection dialog groups engines by type (relational, analytical, document, wide-column, time series, distributed) and has the fields each engine needs: Oracle service name or SID, DuckDB file, Cassandra keyspace, InfluxDB organization, bucket and token, ODBC driver and options, H2 database path
- Monogram icons for the new engines, with the source and license of every logo in `assets/logos/SOURCES.md`

### Fixed

- Postgres: when a result stopped at the row limit, the cancel request for the rest of it could cancel the next statement instead

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

[Unreleased]: https://github.com/hayes-roach/DataBug/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hayes-roach/DataBug/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hayes-roach/DataBug/releases/tag/v0.1.0
