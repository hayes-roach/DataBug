# Development

Building DataBug, how it's put together, and how it's tested. See also [CONTRIBUTING.md](../CONTRIBUTING.md). Back to the [README](../README.md).

## Build and run

Requires Rust 1.89 or newer and a C compiler (for bundled SQLite and the vendored D-Bus used for the Linux keychain). No other system packages are needed on macOS, Windows (MSVC or GNU) or Linux.

```sh
cargo run --release
```

On first launch, choose **New connection**. (`crates/databug-app/assets/sample.sql` is kept only as a test fixture for CI and `--screenshot`.)

To build with only some drivers:

```sh
cargo build --release -p databug --no-default-features --features databug-drivers/postgres,databug-drivers/sqlite
```

Add `--features kerberos` for SQL Server Windows authentication on macOS/Linux (see [SQL Server sign-in](#sql-server-sign-in)); Windows always has it.

For a macOS app bundle: `cargo install cargo-bundle`, then `cd crates/databug-app && cargo bundle --release --features kerberos`. On Linux, `packaging/linux/install.sh` installs the binary, icon and menu entry for your user.

Settings live in your platform's config directory (`~/.config/databug` on Linux, `~/Library/Application Support/dev.DataBug.DataBug` on macOS, `%APPDATA%\DataBug\DataBug\config` on Windows). Set `DATABUG_HOME` to keep everything in one folder instead.

## Architecture

```
crates/
  databug-core     Shared types: connection config, schema catalog, compact ResultSet, plans
  databug-sql      SQL intelligence with no UI or driver deps: tolerant lexer, statement
                  splitter (handles BEGIN…END blocks, GO, $$ bodies), completion, inspections,
                  safety checks, formatter, Mongo shell parser, per-dialect DDL generation,
                  schema diff, rename refactoring
  databug-drivers  One async trait (DbConnection) with an implementation per engine,
                  each behind a cargo feature
  databug-app      egui/eframe desktop app: sessions on background tasks, explorer, editor,
                  grid, plan view, table editor, schema compare, ER diagram, CSV import,
                  rename dialog
```

Each console owns its own session (one connection on a background task), so a long query in one tab never blocks another. Each connection also has a separate metadata session for introspection. The UI talks to sessions over channels and is woken by the session when events arrive.

`databug-sql` is deliberately standalone so it can back a language server: wrapping it in `tower-lsp` would give the same completion and inspections in VS Code, Neovim or Zed.

## Tests

```sh
cargo test --workspace                       # unit tests + SQLite end-to-end
DATABUG_TEST_PG=localhost:5432:store:postgres \
DATABUG_TEST_MYSQL=127.0.0.1:3306:store:root:pw \
cargo test --workspace                       # adds live Postgres and MySQL tests
```

The other engines have live tests that run when their variable is set; with Docker:

| Variable | Server |
|---|---|
| `DATABUG_DUCKDB_LIB=/path/to/libduckdb.dylib` | DuckDB's library from its [releases](https://github.com/duckdb/duckdb/releases/tag/v2.0.0) (no server) |
| `DATABUG_TEST_DUCKDB_DOWNLOAD=1` | downloads and verifies the library the way the dialog does, into a temporary folder |
| `DATABUG_TEST_COCKROACH=localhost:26257:defaultdb:root` | `docker run -d -p 26257:26257 cockroachdb/cockroach:latest-v24.3 start-single-node --insecure` |
| `DATABUG_TEST_TIMESCALE=localhost:5432:postgres:postgres` | `docker run -d -p 5432:5432 -e POSTGRES_HOST_AUTH_METHOD=trust timescale/timescaledb:latest-pg16` |
| `DATABUG_TEST_QUESTDB=localhost:8812:qdb:admin:quest` | `docker run -d -p 8812:8812 questdb/questdb` |
| `DATABUG_TEST_H2=localhost:5435:~/store:sa:sa` | `java -cp h2.jar org.h2.tools.Server -pg -ifNotExists` |
| `DATABUG_TEST_CASSANDRA=localhost:9042` | `docker run -d -p 9042:9042 cassandra:5` |
| `DATABUG_TEST_COUCHDB=localhost:5984:admin:pw` | `docker run -d -p 5984:5984 -e COUCHDB_USER=admin -e COUCHDB_PASSWORD=pw couchdb:3` |
| `DATABUG_TEST_REDIS=localhost:6379::pw` (host:port:user:password; the user may be empty) | `docker run -d -p 6379:6379 redis:8 redis-server --requirepass pw` (uses database 15 and `databug:` keys only) |
| `DATABUG_TEST_INFLUXDB=localhost:8086:example:store:dev-token` | `docker run -d -p 8086:8086 -e DOCKER_INFLUXDB_INIT_MODE=setup -e DOCKER_INFLUXDB_INIT_USERNAME=admin -e DOCKER_INFLUXDB_INIT_PASSWORD=password123 -e DOCKER_INFLUXDB_INIT_ORG=example -e DOCKER_INFLUXDB_INIT_BUCKET=store -e DOCKER_INFLUXDB_INIT_ADMIN_TOKEN=dev-token influxdb:2.7` |
| `DATABUG_TEST_ORACLE`, `DATABUG_TEST_DB2`, `DATABUG_TEST_SPARK` | need the client software above |

The live tests cover streaming, NULLs, row limits, affected-row counts, server messages, error messages, introspection, explain (including rolled-back `EXPLAIN ANALYZE` of a write) and cancelling a running query. They also check every object kind: procedures, overloaded functions, materialized views, triggers, sequences, indexes and foreign keys are introspected, and each definition is fetched and **run back** against the server. Generated `ALTER` scripts (rename, type change, add column, index, table rename, SQLite rebuild) are executed and the schema reloaded to confirm the result, and a Postgres schema compare migration is applied and re-compared to zero differences. CI (`.github/workflows/ci.yml`) runs them against Postgres 16 and MySQL 8.4 and builds on macOS and Windows.

`databug --screenshot out.png` drives the UI headlessly (open sample, run a query, open completion) and saves a PNG. `DATABUG_SHOT_SCENE` picks other scenes: `plan`, `dialog`, `dialog-engines` (engine list open), `light`, `flow` (types a query with completion and runs it), `error`, `confirm`, `tx`, `edit`, `ms-dialog`, `pg-schemas`, `find`, `theme`, `multidb`, `compare`, `editors` (the toolbar's consoles list), `goto` (go to definition), `usages` (other uses of the name at the cursor), `tabmenu`, `couch-docs` / `couch-fields` (the document view; these need `DATABUG_HOME` whose first connection is a CouchDB server with a `store` database holding `photo:0001` and `product:001`) and `redis` (a Redis console with field completion; the first connection is a Redis server with the sample keys). Scenes against a live server take its password from `DATABUG_SHOT_PASSWORD`, kept in memory for that run. `DATABUG_CUSTOM_FRAME=1` shows the Linux title bar on any platform. The demo video comes from `DATABUG_SHOT_SCENE=demo databug --screenshot <folder>`, which plays a scripted tour and saves every frame; `src/demo.rs` explains the connections it expects and the `ffmpeg` command that turns the frames into `docs/demo.mp4`.

`DATABUG_TEST_KRB="user@REALM:password"` with `--features databug-drivers/kerberos` adds a test that signs in to a Kerberos realm through the SQL Server Windows-credentials path.
