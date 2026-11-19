# Roadmap

What isn't done yet. Back to the [README](../README.md).

- The Linux title bar (DataBug's own window buttons, dragging and edge resizing) has only been checked on macOS with `DATABUG_CUSTOM_FRAME=1`; reports from GNOME, KDE and other Wayland or X11 desktops are welcome.
- MongoDB still hasn't run against a live server. SQL Server has been tested against a live server; its less common objects (CLR objects, sequences, synonyms) and `CREATE OR ALTER` editing have seen the least use, so reports are welcome.
- SSH tunnels, client certificates, and cloud sign-in (AWS IAM, Microsoft Entra ID).
- SQL Server Windows authentication hasn't been tried against a real domain. The macOS/Linux `kinit` step was tested against a local Kerberos realm (right and wrong passwords); the handshake with SQL Server itself is tiberius' and is untested here.
- Schema visibility doesn't skip hidden schemas during introspection yet, so very large servers still load everything once.
- Certificate verification mode (`verify-full`) for Postgres TLS; MySQL "prefer" TLS.
- Schema compare across different engines compares types textually, so Postgres `text` vs MySQL `TEXT` matches but `int8` vs `BIGINT` mapping across engines is basic.
- Rename refactoring rewrites references it can resolve from SQL text; dynamic SQL inside routines isn't touched.
- A `databug-lsp` crate and an MCP server for AI agents.
- Oracle, IBM Db2 and Spark SQL ship in every build but haven't been run against a server: they need Oracle Instant Client or an ODBC driver that couldn't be tested here. Reports are welcome.
- CouchDB updates and deletes, InfluxDB 3 SQL (InfluxQL works through its v1 API), Cassandra materialized-view editing and TimescaleDB policies and jobs in the explorer.
- More engines (ClickHouse, Valkey and Redis Cluster) behind the same `DbConnection` trait. Redis connects to a single server; cluster and Sentinel setups aren't supported yet.
- The UI is pinned to egui 0.31; moving to the current egui release is a mechanical upgrade.
