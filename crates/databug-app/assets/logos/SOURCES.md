# Engine logos

These files are compiled into DataBug (`src/icons.rs`) and shipped in the
`logos` folder of every release. Product names and logos are trademarks of
their owners and are used only to identify the database a connection uses.

| File | Source | License |
|---|---|---|
| `postgresql.svg`, `mysql.svg`, `sqlite.svg`, `mongodb.svg`, `couchdb.svg` | Added in the initial commit with no recorded source; the artwork matches the [Devicon](https://github.com/devicons/devicon) icon set | Devicon is MIT licensed; the marks remain trademarks of their projects |
| `sqlserver.png`, `mariadb.png` | Added in the initial commit with no recorded source | Unknown: confirm before redistributing outside DataBug |
| `oracle.svg`, `db2.svg`, `duckdb.svg`, `cassandra.svg`, `cockroachdb.svg`, `timescaledb.svg`, `questdb.svg`, `influxdb.svg`, `spark.svg`, `h2.svg` | Original DataBug monogram tiles (engine color + two letters drawn as strokes), made for this project | Apache-2.0, like the rest of DataBug |

The official logos of Oracle, IBM Db2, DuckDB, Apache Cassandra, CockroachDB,
TimescaleDB, QuestDB, InfluxDB, Apache Spark and H2 are not included: their
trademark policies or missing artwork licenses don't clearly allow bundling
them in another product. To use one locally, put `<name>.svg` or `.png`
(e.g. `oracle.svg`) in the `logos` folder next to DataBug; it replaces the
built-in tile.
