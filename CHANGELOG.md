# Changelog

All notable changes to DataBug are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** in the same pull request as the change, in the section that fits: Added, Changed, Deprecated, Removed, Fixed or Security.

## [Unreleased]

### Added

- Contributing guide, code of conduct, security policy, issue and pull request templates
- New engines: CockroachDB, TimescaleDB, QuestDB and H2 (PostgreSQL protocol, each with its own introspection), DuckDB (embedded), Apache Cassandra (CQL), CouchDB (Mango queries in shell syntax) and InfluxDB (InfluxQL, Flux and line protocol), all in the default build
- Opt-in drivers for Oracle (`oracle` feature, needs Oracle Instant Client), IBM Db2 (`db2`) and Spark SQL through its Thrift server (`spark`), both over ODBC; not yet tested against a live server
- The new-connection dialog groups engines by type (relational, analytical, document, wide-column, time series, distributed) and has the fields each engine needs: Oracle service name or SID, DuckDB file, Cassandra keyspace, InfluxDB organization, bucket and token, ODBC driver and options, H2 database path
- Monogram icons for the new engines, with the source and license of every logo in `assets/logos/SOURCES.md`

### Fixed

- Postgres: when a result stopped at the row limit, the cancel request for the rest of it could cancel the next statement instead

### Changed

- README screenshots updated, with new ones for multiple databases, find and replace, and themes
- SQL Server marked as tested against a live server

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

[Unreleased]: https://github.com/hayes-roach/DataBug/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hayes-roach/DataBug/releases/tag/v0.1.0
