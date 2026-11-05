# Changelog

All notable changes to DataBug are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** in the same pull request as the change, in the section that fits: Added, Changed, Deprecated, Removed, Fixed or Security.

## [Unreleased]

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

[Unreleased]: https://github.com/hayes-roach/DataBug/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hayes-roach/DataBug/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hayes-roach/DataBug/releases/tag/v0.1.0
