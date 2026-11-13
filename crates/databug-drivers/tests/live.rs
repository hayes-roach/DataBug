//! End-to-end driver tests. SQLite always runs; Postgres runs when
//! `DATABUG_TEST_PG=host:port:database:user` is set, and the other servers
//! when their `DATABUG_TEST_*` variable is (see each test).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use databug_core::{ConnectionConfig, DbKind, Dialect, QueryEvent, ResultSet};
use databug_core::ObjectKind;
use databug_drivers::{connect, DbConnection, ExecOptions, Sink, SourceRef};

#[derive(Default)]
struct Collected {
    columns: Vec<String>,
    types: Vec<String>,
    rows: ResultSet,
    truncated: bool,
    affected: Option<u64>,
    messages: Vec<String>,
}

fn sink() -> (Sink, Arc<Mutex<Collected>>) {
    let c: Arc<Mutex<Collected>> = Arc::default();
    let c2 = c.clone();
    let s: Sink = Arc::new(move |ev| {
        let mut c = c2.lock().unwrap();
        match ev {
            QueryEvent::Columns(cols) => {
                c.columns = cols.iter().map(|c| c.name.clone()).collect();
                c.types = cols.iter().map(|c| c.type_name.clone()).collect();
                c.rows = ResultSet::new(cols.len());
            }
            QueryEvent::Rows(r) => {
                c.rows.append(r);
            }
            QueryEvent::Truncated => c.truncated = true,
            QueryEvent::Affected(n) => c.affected = Some(n),
            QueryEvent::Message(m) => c.messages.push(m),
        }
    });
    (s, c)
}

async fn run(conn: &mut Box<dyn DbConnection>, sql: &str, limit: usize) -> anyhow::Result<Collected> {
    let (s, c) = sink();
    conn.execute(sql, &ExecOptions { row_limit: limit }, &s).await?;
    drop(s);
    Ok(Arc::try_unwrap(c).ok().unwrap().into_inner().unwrap())
}

const SAMPLE: &str = include_str!("../../databug-app/assets/sample.sql");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_end_to_end() {
    let dir = std::env::temp_dir().join(format!("databug-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.db");
    let _ = std::fs::remove_file(&path);
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.path = path.to_string_lossy().into_owned();
    let mut conn = connect(&cfg, None).await.unwrap();

    for r in databug_sql::split_statements(SAMPLE, Dialect::Sqlite) {
        run(&mut conn, &SAMPLE[r], 100).await.unwrap();
    }

    let c = run(&mut conn, "SELECT id, name, email FROM customers ORDER BY id LIMIT 3", 100).await.unwrap();
    assert_eq!(c.columns, vec!["id", "name", "email"]);
    assert_eq!(c.rows.rows(), 3);
    assert_eq!(c.rows.cell(0, 0), Some("1"));
    assert_eq!(c.types[1], "text");

    // NULLs survive the round trip
    let c = run(&mut conn, "SELECT NULL AS a, 'x' AS b", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), None);
    assert_eq!(c.rows.cell(0, 1), Some("x"));

    // row limit cuts off and reports it; exact-limit results are not flagged
    let c = run(&mut conn, "SELECT * FROM orders", 1000).await.unwrap();
    assert_eq!(c.rows.rows(), 1000);
    assert!(c.truncated);
    let c = run(&mut conn, "SELECT * FROM orders LIMIT 1000", 1000).await.unwrap();
    assert!(!c.truncated);

    let c = run(&mut conn, "UPDATE products SET stock = stock + 1 WHERE id <= 10", 10).await.unwrap();
    assert_eq!(c.affected, Some(10));

    let cat = conn.introspect().await.unwrap();
    let orders = cat.find_table(None, "orders").unwrap();
    assert_eq!(orders.columns.len(), 5);
    assert!(orders.column("id").unwrap().primary_key);
    assert!(cat.find_table(None, "customer_totals").is_some());

    let plan = conn.explain("SELECT * FROM orders WHERE customer_id = 3", false).await.unwrap();
    let root = plan.root.unwrap();
    assert!(root.children[0].label.contains("idx_orders_customer"), "{:?}", root);

    // cancel a long-running query from another task
    let cancel = conn.canceller().unwrap();
    let start = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel();
    });
    let r = run(&mut conn, "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT count(*) FROM n", 10).await;
    assert!(r.is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
    // connection still usable afterwards
    let c = run(&mut conn, "SELECT count(*) FROM customers", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("500"));
}

fn pg_cfg() -> Option<ConnectionConfig> {
    let v = std::env::var("DATABUG_TEST_PG").ok()?;
    let p: Vec<&str> = v.split(':').collect();
    let mut cfg = ConnectionConfig::new(DbKind::Postgres);
    cfg.host = p[0].into();
    cfg.port = p[1].parse().ok()?;
    cfg.database = p[2].into();
    cfg.user = p[3].into();
    Some(cfg)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_end_to_end() {
    let Some(cfg) = pg_cfg() else {
        eprintln!("DATABUG_TEST_PG not set; skipping");
        return;
    };
    let mut conn = connect(&cfg, None).await.unwrap();

    let c = run(&mut conn, "SELECT id, name, created_at FROM customers ORDER BY id LIMIT 2", 100).await.unwrap();
    assert_eq!(c.rows.rows(), 2);
    assert_eq!(c.types, vec!["int4", "text", "text"]);

    let c = run(&mut conn, "SELECT g FROM generate_series(1, 200000) g", 5000).await.unwrap();
    assert_eq!(c.rows.rows(), 5000);
    assert!(c.truncated);
    // connection still fine after the over-limit cancel
    tokio::time::sleep(Duration::from_millis(100)).await;
    let c = run(&mut conn, "SELECT 42", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("42"));

    let c = run(&mut conn, "DO $$ BEGIN RAISE NOTICE 'hello from plpgsql'; END $$", 10).await.unwrap();
    assert!(c.messages.iter().any(|m| m.contains("hello from plpgsql")), "{:?}", c.messages);

    let err = run(&mut conn, "SELECT * FROM missing_table", 10).await.err().unwrap();
    assert!(err.to_string().contains("does not exist"));

    let cat = conn.introspect().await.unwrap();
    assert_eq!(cat.default_schema, "public");
    assert!(cat.find_table(Some("audit"), "events").unwrap().column("payload").is_some());
    assert!(cat.find_table(None, "orders").unwrap().column("id").unwrap().primary_key);

    let plan = conn.explain("SELECT c.name, sum(o.total) FROM orders o JOIN customers c ON c.id = o.customer_id GROUP BY c.name", true).await.unwrap();
    let root = plan.root.unwrap();
    assert!(root.actual_ms.is_some());
    assert!(plan.raw.contains("Plan"));

    // EXPLAIN ANALYZE of a write is rolled back
    conn.explain("DELETE FROM order_items WHERE id <= 5", true).await.unwrap();
    let c = run(&mut conn, "SELECT count(*) FROM order_items WHERE id <= 5", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("5"));

    let cancel = conn.canceller().unwrap();
    let start = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel();
    });
    let err = run(&mut conn, "SELECT pg_sleep(20)", 10).await.err().unwrap();
    assert!(err.to_string().contains("cancel"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_end_to_end() {
    let Ok(v) = std::env::var("DATABUG_TEST_MYSQL") else {
        eprintln!("DATABUG_TEST_MYSQL not set; skipping");
        return;
    };
    let p: Vec<&str> = v.split(':').collect();
    let mut cfg = ConnectionConfig::new(DbKind::MySql);
    cfg.host = p[0].into();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].into();
    cfg.user = p[3].into();
    let mut conn = connect(&cfg, Some(p[4])).await.unwrap();

    let c = run(&mut conn, "SELECT id, name, NULL AS nothing, price FROM products ORDER BY id LIMIT 2", 100).await.unwrap();
    assert_eq!(c.columns, vec!["id", "name", "nothing", "price"]);
    assert_eq!(c.rows.cell(0, 0), Some("1"));
    assert_eq!(c.rows.cell(0, 2), None);
    assert_eq!(c.types[3], "newdecimal");

    let c = run(&mut conn, "SELECT * FROM customers", 100).await.unwrap();
    assert_eq!(c.rows.rows(), 100);
    assert!(c.truncated);

    let c = run(&mut conn, "UPDATE products SET stock = stock + 1 WHERE id <= 4", 10).await.unwrap();
    assert_eq!(c.affected, Some(4));

    // multiple result sets in one call
    let c = run(&mut conn, "SELECT 1 AS a; SELECT 2 AS b", 10).await;
    assert!(c.is_ok());

    let err = run(&mut conn, "SELECT * FROM nope", 10).await.err().unwrap();
    assert!(err.to_string().contains("1146"), "{err}");

    let cat = conn.introspect().await.unwrap();
    assert_eq!(cat.default_schema, "store");
    assert!(cat.find_table(None, "orders").unwrap().column("id").unwrap().primary_key);

    let plan = conn.explain("SELECT * FROM orders WHERE customer_id = 3", false).await.unwrap();
    assert!(plan.root.is_some());
    assert!(plan.raw.contains("query_block"));

    let cancel = conn.canceller().unwrap();
    let start = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel();
    });
    let _ = run(&mut conn, "SELECT SLEEP(20)", 10).await;
    assert!(start.elapsed() < Duration::from_secs(5));
    let c = run(&mut conn, "SELECT 7", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("7"));
}

/// Fetch an object's definition and run it back, statement by statement.
async fn round_trip(conn: &mut Box<dyn DbConnection>, d: Dialect, schema: &str, r: &SourceRef) -> String {
    let src = conn.object_source(schema, r).await.unwrap_or_else(|e| panic!("source of {}: {e}", r.name));
    for st in databug_sql::split_statements(&src, d) {
        run(conn, &src[st], 10).await.unwrap_or_else(|e| panic!("re-running {}: {e}\n{src}", r.name));
    }
    src
}

fn find_obj<'a>(cat: &'a databug_core::Catalog, schema: &str, kind: ObjectKind, name: &str) -> &'a databug_core::DbObject {
    cat.schema(schema).unwrap().objects.iter().find(|o| o.kind == kind && o.name == name).unwrap_or_else(|| panic!("{name} ({kind:?}) not introspected"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_objects() {
    let dir = std::env::temp_dir().join(format!("databug-obj-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("o.db");
    let _ = std::fs::remove_file(&path);
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.path = path.to_string_lossy().into_owned();
    let mut conn = connect(&cfg, None).await.unwrap();
    for r in databug_sql::split_statements(SAMPLE, Dialect::Sqlite) {
        run(&mut conn, &SAMPLE[r], 10).await.unwrap();
    }
    run(&mut conn, "CREATE TRIGGER trg_stock AFTER UPDATE ON products BEGIN UPDATE products SET stock = stock WHERE id = NEW.id AND 0; END", 10).await.unwrap();
    let cat = conn.introspect().await.unwrap();
    let oi = cat.find_table(None, "order_items").unwrap();
    assert_eq!(oi.foreign_keys.len(), 2, "{:?}", oi.foreign_keys);
    assert!(oi.foreign_keys.iter().any(|f| f.ref_table == "orders" && f.columns == vec!["order_id"]));
    let orders = cat.find_table(None, "orders").unwrap();
    assert!(orders.indexes.iter().any(|i| i.name == "idx_orders_customer" && i.columns == vec!["customer_id"]));
    assert!(orders.indexes.iter().any(|i| i.primary));
    let trg = find_obj(&cat, "main", ObjectKind::Trigger, "trg_stock");
    assert_eq!(trg.table.as_deref(), Some("products"));
    let src = round_trip(&mut conn, Dialect::Sqlite, "main", &SourceRef::relation("customer_totals", ObjectKind::View)).await;
    assert!(src.contains("CREATE VIEW customer_totals"));
    round_trip(&mut conn, Dialect::Sqlite, "main", &SourceRef::from_object(trg)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_objects() {
    let Some(cfg) = pg_cfg() else { return };
    let mut conn = connect(&cfg, None).await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS objtest CASCADE",
        "CREATE SCHEMA objtest",
        "CREATE TABLE objtest.parent (id int PRIMARY KEY, name text)",
        "CREATE TABLE objtest.child (id int PRIMARY KEY, parent_id int REFERENCES objtest.parent(id) ON DELETE CASCADE, qty int)",
        "CREATE INDEX child_parent_idx ON objtest.child (parent_id, qty)",
        "CREATE MATERIALIZED VIEW objtest.totals AS SELECT parent_id, sum(qty) AS qty FROM objtest.child GROUP BY parent_id",
        "CREATE FUNCTION objtest.double_it(x int) RETURNS int LANGUAGE sql AS $$ SELECT x * 2 $$",
        "CREATE FUNCTION objtest.double_it(x numeric) RETURNS numeric LANGUAGE sql AS $$ SELECT x * 2 $$",
        "CREATE PROCEDURE objtest.bump(n int) LANGUAGE plpgsql AS $$ BEGIN UPDATE objtest.child SET qty = qty + n; END $$",
        "CREATE FUNCTION objtest.touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$",
        "CREATE TRIGGER child_touch BEFORE UPDATE ON objtest.child FOR EACH ROW EXECUTE FUNCTION objtest.touch()",
        "CREATE SEQUENCE objtest.ticket_seq START 100",
        "CREATE VIEW objtest.parent_names AS SELECT id, upper(name) AS name FROM objtest.parent",
    ] {
        run(&mut conn, sql, 10).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let cat = conn.introspect().await.unwrap();
    let s = cat.schema("objtest").unwrap();
    let child = s.table("child").unwrap();
    assert_eq!(child.foreign_keys[0].ref_table, "parent");
    assert_eq!(child.foreign_keys[0].on_delete, "CASCADE");
    assert!(child.indexes.iter().any(|i| i.name == "child_parent_idx" && i.columns == vec!["parent_id", "qty"]));
    assert_eq!(s.table("totals").unwrap().kind, ObjectKind::MaterializedView);
    let overloads: Vec<_> = s.objects.iter().filter(|o| o.name == "double_it").collect();
    assert_eq!(overloads.len(), 2, "both overloads listed");
    for kind_name in [(ObjectKind::Procedure, "bump"), (ObjectKind::Trigger, "child_touch"), (ObjectKind::Sequence, "ticket_seq")] {
        find_obj(&cat, "objtest", kind_name.0, kind_name.1);
    }
    for o in s.objects.clone() {
        let src = round_trip(&mut conn, Dialect::Postgres, "objtest", &SourceRef::from_object(&o)).await;
        assert!(!src.trim().is_empty());
    }
    let src = round_trip(&mut conn, Dialect::Postgres, "objtest", &SourceRef::relation("parent_names", ObjectKind::View)).await;
    assert!(src.starts_with("CREATE OR REPLACE VIEW objtest.parent_names"), "{src}");
    round_trip(&mut conn, Dialect::Postgres, "objtest", &SourceRef::relation("totals", ObjectKind::MaterializedView)).await;
    run(&mut conn, "DROP SCHEMA objtest CASCADE", 10).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_objects() {
    let Ok(v) = std::env::var("DATABUG_TEST_MYSQL") else { return };
    let p: Vec<&str> = v.split(':').collect();
    let mut cfg = ConnectionConfig::new(DbKind::MySql);
    cfg.host = p[0].into();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].into();
    cfg.user = p[3].into();
    let mut conn = connect(&cfg, Some(p[4])).await.unwrap();
    let script = "DROP TABLE IF EXISTS obj_child;\nDROP TABLE IF EXISTS obj_parent;\n\
        CREATE TABLE obj_parent (id INT PRIMARY KEY, name VARCHAR(40));\n\
        CREATE TABLE obj_child (id INT PRIMARY KEY, parent_id INT, qty INT, KEY ix_parent (parent_id), CONSTRAINT fk_child_parent FOREIGN KEY (parent_id) REFERENCES obj_parent(id) ON DELETE CASCADE);\n\
        DROP PROCEDURE IF EXISTS obj_bump;\n\
        CREATE PROCEDURE obj_bump(IN n INT)\nBEGIN\n  IF n > 0 THEN\n    UPDATE obj_child SET qty = qty + n;\n  ELSE\n    UPDATE obj_child SET qty = 0;\n  END IF;\nEND;\n\
        DROP FUNCTION IF EXISTS obj_double;\n\
        CREATE FUNCTION obj_double(x INT) RETURNS INT DETERMINISTIC RETURN x * 2;\n\
        CREATE OR REPLACE VIEW obj_names AS SELECT id, UPPER(name) AS name FROM obj_parent;\n\
        DROP TRIGGER IF EXISTS obj_trg;\n\
        CREATE TRIGGER obj_trg BEFORE UPDATE ON obj_child FOR EACH ROW SET NEW.qty = GREATEST(NEW.qty, 0);";
    for r in databug_sql::split_statements(script, Dialect::MySql) {
        run(&mut conn, &script[r.clone()], 10).await.unwrap_or_else(|e| panic!("{}: {e}", &script[r]));
    }
    let cat = conn.introspect().await.unwrap();
    let child = cat.find_table(None, "obj_child").unwrap();
    assert_eq!(child.foreign_keys[0].name, "fk_child_parent");
    assert_eq!(child.foreign_keys[0].on_delete, "CASCADE");
    assert!(child.indexes.iter().any(|i| i.name == "ix_parent"));
    let db = cat.default_schema.clone();
    for (k, n) in [(ObjectKind::Procedure, "obj_bump"), (ObjectKind::Function, "obj_double"), (ObjectKind::Trigger, "obj_trg")] {
        let o = find_obj(&cat, &db, k, n).clone();
        round_trip(&mut conn, Dialect::MySql, &db, &SourceRef::from_object(&o)).await;
    }
    let src = round_trip(&mut conn, Dialect::MySql, &db, &SourceRef::relation("obj_names", ObjectKind::View)).await;
    assert!(src.starts_with("CREATE OR REPLACE"), "{src}");
    run(&mut conn, "CALL obj_bump(2)", 10).await.unwrap();
}

use databug_sql::ddl::{self, ColumnEdit};
use databug_sql::diff::{self, Sources};

async fn run_script(conn: &mut Box<dyn DbConnection>, d: Dialect, sql: &str) {
    for r in databug_sql::split_statements(sql, d) {
        run(conn, &sql[r.clone()], 10).await.unwrap_or_else(|e| panic!("{}: {e}\n--- script ---\n{sql}", &sql[r]));
    }
}

/// Rename a column, change a type, add a column and an index, and rename the
/// table — then reload and check the database agrees.
async fn alter_round_trip(conn: &mut Box<dyn DbConnection>, d: Dialect, schema: &str) {
    run_script(conn, d, "DROP TABLE IF EXISTS gadgets_renamed; DROP TABLE IF EXISTS gadgets; CREATE TABLE gadgets (id INTEGER PRIMARY KEY, label VARCHAR(40), qty INTEGER); INSERT INTO gadgets VALUES (1, 'a', 2)").await;
    let cat = conn.introspect().await.unwrap();
    let t = cat.schema(schema).unwrap().table("gadgets").unwrap().clone();
    let mut edits: Vec<ColumnEdit> = t.columns.iter().map(|c| ColumnEdit { original: Some(c.name.clone()), column: c.clone(), dropped: false }).collect();
    edits[1].column.name = "title".into();
    edits[2].column.data_type = "BIGINT".into();
    edits[2].column.nullable = false;
    edits.push(ColumnEdit { original: None, column: databug_core::ColumnInfo { name: "note".into(), data_type: "VARCHAR(100)".into(), nullable: true, primary_key: false, default: None }, dropped: false });
    let mut idx = t.indexes.clone();
    idx.push(databug_core::IndexInfo { name: "gadgets_title_ix".into(), columns: vec!["title".into()], unique: false, primary: false });
    let stmts = ddl::alter_table(d, schema, &t, "gadgets_renamed", &edits, &idx, &[]);
    let sql = ddl::script(d, &stmts);
    run_script(conn, d, &sql).await;
    let cat = conn.introspect().await.unwrap();
    let t = cat.schema(schema).unwrap().table("gadgets_renamed").unwrap_or_else(|| panic!("rename failed:\n{sql}"));
    assert!(t.column("title").is_some(), "{sql}");
    let qty = t.column("qty").unwrap();
    assert!(qty.data_type.to_lowercase().contains("big") || qty.data_type == "int8", "type now {}", qty.data_type);
    assert!(!qty.nullable);
    assert!(t.column("note").is_some());
    assert!(t.indexes.iter().any(|i| i.columns == vec!["title"]), "{:?}", t.indexes);
    let c = run(conn, "SELECT title, qty FROM gadgets_renamed", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("a"), "data survived");
    run(conn, "DROP TABLE gadgets_renamed", 10).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_alter_and_rebuild() {
    let dir = std::env::temp_dir().join(format!("databug-alter-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.db");
    let _ = std::fs::remove_file(&path);
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.path = path.to_string_lossy().into_owned();
    let mut conn = connect(&cfg, None).await.unwrap();
    alter_round_trip(&mut conn, Dialect::Sqlite, "main").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_alter_and_compare() {
    let Some(cfg) = pg_cfg() else { return };
    let mut conn = connect(&cfg, None).await.unwrap();
    alter_round_trip(&mut conn, Dialect::Postgres, "public").await;

    // two schemas that differ; migrate the target and compare again
    run_script(&mut conn, Dialect::Postgres, "DROP SCHEMA IF EXISTS cmp_a CASCADE; DROP SCHEMA IF EXISTS cmp_b CASCADE; CREATE SCHEMA cmp_a; CREATE SCHEMA cmp_b;
        CREATE TABLE cmp_a.parent (id int PRIMARY KEY, name text NOT NULL);
        CREATE TABLE cmp_a.child (id int PRIMARY KEY, parent_id int REFERENCES cmp_a.parent(id), amount numeric(10,2), created timestamp DEFAULT now());
        CREATE INDEX child_amount_ix ON cmp_a.child (amount);
        CREATE VIEW cmp_a.big AS SELECT id FROM cmp_a.child WHERE amount > 100;
        CREATE FUNCTION cmp_a.twice(x int) RETURNS int LANGUAGE sql AS $$ SELECT x * 2 $$;
        CREATE TABLE cmp_b.parent (id int PRIMARY KEY, name text);
        CREATE TABLE cmp_b.child (id int PRIMARY KEY, parent_id int, amount int, legacy text);
        CREATE TABLE cmp_b.obsolete (id int);").await;
    let cat = conn.introspect().await.unwrap();
    let mut src_sources = Sources::new();
    for o in cat.schema("cmp_a").unwrap().objects.clone() {
        src_sources.insert(diff::source_key(o.kind, &o.name, &o.detail), conn.object_source("cmp_a", &SourceRef::from_object(&o)).await.unwrap().replace("cmp_a.", "cmp_b."));
    }
    let v = conn.object_source("cmp_a", &SourceRef::relation("big", ObjectKind::View)).await.unwrap();
    src_sources.insert(diff::source_key(ObjectKind::View, "big", ""), v);
    let items = diff::diff(&cat, "cmp_a", &cat, "cmp_b", Dialect::Postgres, &src_sources, &Sources::new());
    assert!(items.len() >= 5, "{:#?}", items.iter().map(|i| (&i.kind, &i.name)).collect::<Vec<_>>());
    let all: Vec<&diff::DiffItem> = items.iter().collect();
    let script = diff::migration_script(Dialect::Postgres, &all);
    run_script(&mut conn, Dialect::Postgres, &script).await;
    let cat = conn.introspect().await.unwrap();
    let again = diff::diff(&cat, "cmp_a", &cat, "cmp_b", Dialect::Postgres, &Sources::new(), &Sources::new());
    assert!(again.is_empty(), "still different after migrating:\n{script}\n{:#?}", again.iter().map(|i| (&i.name, &i.details)).collect::<Vec<_>>());
    run_script(&mut conn, Dialect::Postgres, "DROP SCHEMA cmp_a CASCADE; DROP SCHEMA cmp_b CASCADE").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_alter() {
    let Ok(v) = std::env::var("DATABUG_TEST_MYSQL") else { return };
    let p: Vec<&str> = v.split(':').collect();
    let mut cfg = ConnectionConfig::new(DbKind::MySql);
    cfg.host = p[0].into();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].into();
    cfg.user = p[3].into();
    let mut conn = connect(&cfg, Some(p[4])).await.unwrap();
    let schema = conn.introspect().await.unwrap().default_schema;
    alter_round_trip(&mut conn, Dialect::MySql, &schema).await;
}

// ---------------------------------------------------------------------------
// Engines added after the first five. DuckDB is embedded and always runs;
// the others run when their variable is set (see CONTRIBUTING.md for Docker
// commands). All of them use the Store sample data or small made-up rows.

fn env_parts(name: &str) -> Option<Vec<String>> {
    match std::env::var(name) {
        Ok(v) => Some(v.split(':').map(str::to_string).collect()),
        Err(_) => {
            eprintln!("{name} not set; skipping");
            None
        }
    }
}

/// Drop the sample tables (children first) and load `sample.sql`.
async fn seed_store(conn: &mut Box<dyn DbConnection>, d: Dialect) {
    seed_store_sql(conn, d, SAMPLE).await
}

async fn seed_store_sql(conn: &mut Box<dyn DbConnection>, d: Dialect, sql: &str) {
    for v in ["customer_totals", "shipped"] {
        let _ = run(conn, &format!("DROP VIEW IF EXISTS {v}"), 10).await;
    }
    for t in ["order_items", "orders", "products", "customers"] {
        if run(conn, &format!("DROP TABLE IF EXISTS {t} CASCADE"), 10).await.is_err() {
            let _ = run(conn, &format!("DROP TABLE IF EXISTS {t}"), 10).await;
        }
    }
    for r in databug_sql::split_statements(sql, d) {
        if let Err(e) = run(conn, &sql[r.clone()], 10).await {
            panic!("seeding failed on {}: {e:#}", &sql[r]);
        }
    }
}

/// `sample.sql` for engines whose `/` turns integers into a fraction (DuckDB,
/// CockroachDB) and that spell integer division `//`.
fn int_division_sample() -> String {
    SAMPLE.replace("(i - 1) / 3", "(i - 1) // 3").replace("(i / 8)", "(i // 8)")
}

/// Checks every SQL engine with the Store sample should pass.
async fn store_checks(conn: &mut Box<dyn DbConnection>) -> databug_core::Catalog {
    let c = run(conn, "SELECT id, name, email FROM customers ORDER BY id LIMIT 3", 100).await.unwrap();
    assert_eq!(c.columns.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>(), vec!["id", "name", "email"]);
    assert_eq!(c.rows.rows(), 3);
    assert_eq!(c.rows.cell(0, 0), Some("1"));
    let c = run(conn, "SELECT NULL AS a, 'x' AS b", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), None);
    assert_eq!(c.rows.cell(0, 1), Some("x"));
    let c = run(conn, "SELECT * FROM orders", 1000).await.unwrap();
    assert_eq!(c.rows.rows(), 1000);
    assert!(c.truncated);
    let c = run(conn, "UPDATE products SET stock = stock + 1 WHERE id <= 4", 10).await.unwrap();
    assert_eq!(c.affected, Some(4));
    let cat = conn.introspect().await.unwrap();
    let orders = cat.find_table(None, "orders").expect("orders in catalog");
    assert!(orders.column("id").unwrap().primary_key, "{orders:?}");
    assert!(orders.primary_index().is_some());
    let ix = orders.indexes.iter().find(|i| i.name.eq_ignore_ascii_case("idx_orders_customer")).unwrap_or_else(|| panic!("{:?}", orders.indexes));
    assert!(ix.columns.iter().any(|c| c.eq_ignore_ascii_case("customer_id")) && !ix.unique, "{ix:?}");
    let items = cat.find_table(None, "order_items").unwrap();
    assert!(items.foreign_keys.iter().any(|f| f.ref_table.eq_ignore_ascii_case("orders")), "{:?}", items.foreign_keys);
    cat
}

/// `DATABUG_DUCKDB_LIB=/path/to/libduckdb.dylib` (or `.so` / `duckdb.dll`):
/// DuckDB's library is downloaded by the app on demand, so tests need one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duckdb_end_to_end() {
    if std::env::var_os("DATABUG_DUCKDB_LIB").is_none() {
        eprintln!("DATABUG_DUCKDB_LIB not set; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("databug-duck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("store.duckdb");
    let _ = std::fs::remove_file(&path);
    let mut cfg = ConnectionConfig::new(DbKind::DuckDb);
    cfg.path = path.to_string_lossy().into_owned();
    let mut conn = connect(&cfg, None).await.unwrap();
    seed_store_sql(&mut conn, Dialect::Postgres, &int_division_sample()).await;
    let cat = store_checks(&mut conn).await;
    assert_eq!(cat.default_schema, "main");

    let c = run(&mut conn, "SELECT price, [1, 2] AS l, DATE '2026-01-02' AS d FROM products WHERE id = 1", 10).await.unwrap();
    assert_eq!(c.types, vec!["decimal(10,2)", "integer[]", "date"]);
    assert_eq!(c.rows.cell(0, 0).map(|p| p.contains('.')), Some(true));
    assert_eq!(c.rows.cell(0, 1), Some("[1, 2]"));
    assert_eq!(c.rows.cell(0, 2), Some("2026-01-02"));

    run(&mut conn, "CREATE OR REPLACE VIEW shipped AS SELECT * FROM orders WHERE status = 'shipped'", 10).await.unwrap();
    run(&mut conn, "CREATE OR REPLACE MACRO with_tax(x) AS x * 1.08", 10).await.unwrap();
    let cat = conn.introspect().await.unwrap();
    assert_eq!(cat.find_table(None, "shipped").unwrap().kind, ObjectKind::View);
    let src = conn.object_source("main", &SourceRef::relation("shipped", ObjectKind::View)).await.unwrap();
    assert!(src.contains("CREATE OR REPLACE VIEW"), "{src}");
    let m = find_obj(&cat, "main", ObjectKind::Function, "with_tax");
    let src = conn.object_source("main", &SourceRef::from_object(m)).await.unwrap();
    assert!(src.starts_with("CREATE OR REPLACE MACRO") && src.contains("1.08"), "{src}");

    let plan = conn.explain("SELECT c.name, sum(o.total) FROM orders o JOIN customers c ON c.id = o.customer_id GROUP BY c.name", false).await.unwrap();
    assert!(!plan.raw.is_empty());
    conn.explain("DELETE FROM order_items WHERE id <= 5", true).await.unwrap();
    let c = run(&mut conn, "SELECT count(*) FROM order_items WHERE id <= 5", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("5"));

    let cancel = conn.canceller().unwrap();
    let start = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel();
    });
    let err = run(&mut conn, "SELECT count(*) FROM range(100000000000) a", 10).await.err().unwrap();
    assert!(err.to_string().to_lowercase().contains("interrupt"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(10));
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

fn pg_wire_cfg(kind: DbKind, p: &[String]) -> (ConnectionConfig, Option<String>) {
    let mut cfg = ConnectionConfig::new(kind);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p.get(2).cloned().unwrap_or_default();
    cfg.user = p.get(3).cloned().unwrap_or_default();
    cfg.ssl = databug_core::SslMode::Disable;
    (cfg, p.get(4).cloned())
}

/// `DATABUG_TEST_COCKROACH=host:port:database:user`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cockroachdb_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_COCKROACH") else { return };
    let (cfg, pw) = pg_wire_cfg(DbKind::CockroachDb, &p);
    let mut conn = connect(&cfg, pw.as_deref()).await.unwrap();
    seed_store_sql(&mut conn, Dialect::Postgres, &int_division_sample()).await;
    let cat = store_checks(&mut conn).await;
    assert_eq!(cat.default_schema, "public");
    assert!(cat.schema("crdb_internal").is_none() && cat.schema("pg_catalog").is_none());
    run(&mut conn, "CREATE OR REPLACE VIEW shipped AS SELECT id, total FROM orders WHERE status = 'shipped'", 10).await.unwrap();
    let src = conn.object_source("public", &SourceRef::relation("shipped", ObjectKind::View)).await.unwrap();
    assert!(src.contains("CREATE OR REPLACE VIEW"), "{src}");
    let plan = conn.explain("SELECT * FROM orders WHERE customer_id = 3", false).await.unwrap();
    assert!(!plan.raw.is_empty());
    let (dbs, current) = conn.list_databases().await.unwrap();
    assert!(dbs.contains(&current), "{dbs:?} {current}");
}

/// `DATABUG_TEST_TIMESCALE=host:port:database:user`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timescaledb_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_TIMESCALE") else { return };
    let (cfg, pw) = pg_wire_cfg(DbKind::TimescaleDb, &p);
    let mut conn = connect(&cfg, pw.as_deref()).await.unwrap();
    run(&mut conn, "CREATE EXTENSION IF NOT EXISTS timescaledb", 10).await.unwrap();
    seed_store(&mut conn, Dialect::Postgres).await;
    store_checks(&mut conn).await;
    let _ = run(&mut conn, "DROP MATERIALIZED VIEW IF EXISTS sales_hourly", 10).await;
    let _ = run(&mut conn, "DROP TABLE IF EXISTS sales", 10).await;
    run(&mut conn, "CREATE TABLE sales (at TIMESTAMPTZ NOT NULL, product_id INT, amount NUMERIC(10,2))", 10).await.unwrap();
    run(&mut conn, "SELECT create_hypertable('sales', 'at')", 10).await.unwrap();
    run(&mut conn, "INSERT INTO sales SELECT TIMESTAMPTZ '2026-01-01' + i * INTERVAL '10 minutes', 1 + i % 60, (i % 50) + 0.99 FROM generate_series(1, 500) i", 10).await.unwrap();
    run(&mut conn, "CREATE MATERIALIZED VIEW sales_hourly WITH (timescaledb.continuous) AS SELECT time_bucket('1 hour', at) AS bucket, product_id, sum(amount) AS total FROM sales GROUP BY 1, 2 WITH NO DATA", 10).await.unwrap();
    let cat = conn.introspect().await.unwrap();
    assert!(cat.schemas.iter().all(|s| !s.name.starts_with("_timescaledb")), "{:?}", cat.schemas.iter().map(|s| &s.name).collect::<Vec<_>>());
    assert_eq!(cat.find_table(None, "sales").unwrap().kind, ObjectKind::Table);
    assert_eq!(cat.find_table(None, "sales_hourly").unwrap().kind, ObjectKind::MaterializedView);
    let src = conn.object_source("public", &SourceRef::relation("sales_hourly", ObjectKind::MaterializedView)).await.unwrap();
    assert!(src.contains("timescaledb.continuous") && src.contains("time_bucket"), "{src}");
    let plan = conn.explain("SELECT * FROM sales WHERE at > '2026-01-02'", false).await.unwrap();
    assert!(plan.root.is_some());
}

/// `DATABUG_TEST_QUESTDB=host:port:database:user:password` (8812, qdb, admin, quest)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn questdb_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_QUESTDB") else { return };
    let (cfg, pw) = pg_wire_cfg(DbKind::QuestDb, &p);
    let mut conn = connect(&cfg, pw.as_deref()).await.unwrap();
    let _ = run(&mut conn, "DROP TABLE IF EXISTS store_sales", 10).await;
    run(&mut conn, "CREATE TABLE store_sales (at TIMESTAMP, sku SYMBOL INDEX, qty INT, price DOUBLE) TIMESTAMP(at) PARTITION BY DAY", 10).await.unwrap();
    run(&mut conn, "INSERT INTO store_sales SELECT timestamp_sequence('2026-01-01', 60000000L), rnd_symbol('SKU-1001', 'SKU-1002', 'SKU-1003'), rnd_int(1, 9, 0), rnd_double() * 100 FROM long_sequence(2000)", 10).await.unwrap();
    // QuestDB applies writes asynchronously through its WAL
    let mut n = String::new();
    for _ in 0..50 {
        let c = run(&mut conn, "SELECT count() FROM store_sales", 10).await.unwrap();
        n = c.rows.cell(0, 0).unwrap_or("").to_string();
        if n == "2000" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(n, "2000");
    let c = run(&mut conn, "SELECT * FROM store_sales", 500).await.unwrap();
    assert_eq!(c.rows.rows(), 500);
    assert!(c.truncated);
    let cat = conn.introspect().await.unwrap();
    let t = cat.find_table(None, "store_sales").unwrap();
    assert_eq!(t.column("sku").unwrap().data_type, "symbol");
    assert!(!t.column("at").unwrap().nullable);
    assert!(t.indexes.iter().any(|i| i.columns == vec!["sku".to_string()]));
    let plan = conn.explain("SELECT sku, sum(qty) FROM store_sales SAMPLE BY 1h", false).await.unwrap();
    assert!(!plan.raw.is_empty());
}

/// `DATABUG_TEST_H2=host:port:database:user:password` for an H2 server
/// started with `-pg` (e.g. `java -cp h2.jar org.h2.tools.Server -pg -ifNotExists`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_H2") else { return };
    let (cfg, pw) = pg_wire_cfg(DbKind::H2, &p);
    let mut conn = connect(&cfg, pw.as_deref()).await.unwrap();
    seed_store(&mut conn, Dialect::Postgres).await;
    let cat = store_checks(&mut conn).await;
    // the PostgreSQL mode folds names to lower case
    assert!(cat.default_schema.eq_ignore_ascii_case("public"), "{}", cat.default_schema);
    assert!(cat.schema("information_schema").is_none() && cat.schema("pg_catalog").is_none());
    run(&mut conn, "CREATE OR REPLACE VIEW shipped AS SELECT id, total FROM orders WHERE status = 'shipped'", 10).await.unwrap();
    let cat = conn.introspect().await.unwrap();
    let view = cat.find_table(None, "shipped").unwrap();
    assert_eq!(view.kind, ObjectKind::View);
    let src = conn.object_source(&cat.default_schema, &SourceRef::relation(&view.name, ObjectKind::View)).await.unwrap();
    assert!(src.contains("CREATE OR REPLACE"), "{src}");
    let plan = conn.explain("SELECT * FROM orders WHERE customer_id = 3", false).await.unwrap();
    assert!(!plan.raw.is_empty());
}

/// `DATABUG_TEST_CASSANDRA=host:port[:user:password]`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cassandra_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_CASSANDRA") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::Cassandra);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.user = p.get(2).cloned().unwrap_or_default();
    cfg.ssl = databug_core::SslMode::Disable;
    let mut conn = connect(&cfg, p.get(3).map(String::as_str)).await.unwrap();
    run(&mut conn, "CREATE KEYSPACE IF NOT EXISTS databug_store WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}", 10).await.unwrap();
    run(&mut conn, "USE databug_store", 10).await.unwrap();
    run(&mut conn, "DROP TABLE IF EXISTS orders", 10).await.unwrap();
    run(&mut conn, "CREATE TABLE orders (customer_id int, id int, status text, total decimal, ordered_at timestamp, PRIMARY KEY (customer_id, id))", 10).await.unwrap();
    run(&mut conn, "CREATE INDEX IF NOT EXISTS orders_status ON orders (status)", 10).await.unwrap();
    for i in 1..=30 {
        run(&mut conn, &format!("INSERT INTO orders (customer_id, id, status, total, ordered_at) VALUES ({}, {i}, '{}', {}.25, '2026-01-{:02}')", 1 + i % 5, if i % 3 == 0 { "pending" } else { "shipped" }, 10 + i, 1 + i % 28), 10).await.unwrap();
    }
    let c = run(&mut conn, "SELECT id, status, total, ordered_at FROM orders WHERE customer_id = 2", 100).await.unwrap();
    assert_eq!(c.columns, vec!["id", "status", "total", "ordered_at"]);
    assert_eq!(c.rows.rows(), 6);
    assert_eq!(c.rows.cell(0, 2), Some("11.25"));
    assert!(c.rows.cell(0, 3).unwrap().starts_with("2026-01-02"));
    let c = run(&mut conn, "SELECT * FROM orders", 10).await.unwrap();
    assert_eq!(c.rows.rows(), 10);
    assert!(c.truncated);
    let cat = conn.introspect().await.unwrap();
    assert_eq!(cat.default_schema, "databug_store");
    assert!(cat.schema("system").is_none());
    let t = cat.find_table(Some("databug_store"), "orders").unwrap();
    assert_eq!(t.primary_key().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["customer_id", "id"]);
    assert!(t.indexes.iter().any(|i| i.name == "orders_status" && i.columns == vec!["status".to_string()]));
    let err = run(&mut conn, "SELECT * FROM nope", 10).await.err().unwrap();
    assert!(err.to_string().to_lowercase().contains("nope"), "{err}");
}

/// `DATABUG_TEST_COUCHDB=host:port:user:password`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn couchdb_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_COUCHDB") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::CouchDb);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.user = p.get(2).cloned().unwrap_or_default();
    cfg.ssl = databug_core::SslMode::Disable;
    let mut conn = connect(&cfg, p.get(3).map(String::as_str)).await.unwrap();
    let _ = run(&mut conn, "db.databug_orders.drop()", 10).await;
    run(&mut conn, "db.createCollection('databug_orders')", 10).await.unwrap();
    let docs: Vec<String> = (1..=40).map(|i| format!("{{_id: 'order-{i}', customer_id: {}, status: '{}', total: {}.5}}", 1 + i % 7, if i % 4 == 0 { "pending" } else { "shipped" }, 10 + i)).collect();
    let c = run(&mut conn, &format!("db.databug_orders.insertMany([{}])", docs.join(", ")), 10).await.unwrap();
    assert_eq!(c.affected, Some(40));
    run(&mut conn, "db.databug_orders.createIndex({total: 1})", 10).await.unwrap();
    let c = run(&mut conn, "db.databug_orders.find({status: 'shipped', total: {$gt: 30}}, {_id: 1, total: 1}).sort({total: -1}).limit(3)", 100).await.unwrap();
    assert_eq!(c.columns, vec!["_id", "total"]);
    assert_eq!(c.rows.rows(), 3);
    assert_eq!(c.rows.cell(0, 0), Some("order-39"));
    let c = run(&mut conn, "db.databug_orders.find({})", 25).await.unwrap();
    assert_eq!(c.rows.rows(), 25);
    assert!(c.truncated);
    let c = run(&mut conn, "db.databug_orders.countDocuments({status: 'pending'})", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("10"));
    let c = run(&mut conn, "GET /databug_orders", 10).await.unwrap();
    assert!(c.columns.contains(&"doc_count".to_string()));
    let cat = conn.introspect().await.unwrap();
    let t = cat.find_table(None, "databug_orders").unwrap();
    assert!(t.column("status").is_some() && t.column("_id").unwrap().primary_key);
    assert!(t.indexes.iter().any(|i| i.columns == vec!["total".to_string()]));
    let plan = conn.explain("db.databug_orders.find({total: {$gt: 30}})", false).await.unwrap();
    assert!(plan.raw.contains("index"));
    run(&mut conn, "db.databug_orders.drop()", 10).await.unwrap();
    if !cfg.user.is_empty() {
        let err = connect(&cfg, Some("not-the-password")).await.err().unwrap();
        assert!(err.to_string().contains("401"), "{err}");
    }
}

/// `DATABUG_TEST_INFLUXDB=host:port:org:bucket:token` (InfluxDB 2.x)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn influxdb_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_INFLUXDB") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::InfluxDb);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.org = p[2].clone();
    cfg.database = p[3].clone();
    cfg.ssl = databug_core::SslMode::Disable;
    let mut conn = connect(&cfg, Some(&p[4])).await.unwrap();
    let lines: Vec<String> = (0..60).map(|i| format!("store_orders,region={} total={}.5,qty={}i {}", if i % 2 == 0 { "east" } else { "west" }, 10 + i, 1 + i % 4, 1_767_225_600_000_000_000i64 + i as i64 * 60_000_000_000)).collect();
    let c = run(&mut conn, &format!("INSERT {}", lines.join("\n")), 10).await.unwrap();
    assert_eq!(c.affected, Some(60));
    let c = run(&mut conn, "SELECT total, qty, region FROM store_orders WHERE time >= '2026-01-01T00:00:00Z' AND time < '2026-01-01T01:00:00Z'", 100).await.unwrap();
    assert_eq!(c.columns, vec!["time", "total", "qty", "region"]);
    assert_eq!(c.rows.rows(), 60);
    let c = run(&mut conn, "SELECT count(total) FROM store_orders WHERE time >= '2026-01-01T00:00:00Z' GROUP BY region", 100).await.unwrap();
    assert!(c.columns.contains(&"region".to_string()));
    let c = run(&mut conn, &format!("from(bucket: \"{}\") |> range(start: 2026-01-01T00:00:00Z, stop: 2026-01-01T01:00:00Z) |> filter(fn: (r) => r._measurement == \"store_orders\" and r._field == \"total\") |> count()", p[3]), 100).await.unwrap();
    assert!(c.columns.contains(&"_value".to_string()), "{:?}", c.columns);
    let cat = conn.introspect().await.unwrap();
    let t = cat.find_table(None, "store_orders").unwrap();
    assert_eq!(t.column("region").unwrap().data_type, "tag");
    assert_eq!(t.column("total").unwrap().data_type, "float");
    assert_eq!(t.column("qty").unwrap().data_type, "integer");
    let err = connect(&cfg, Some("not-the-token")).await.err().unwrap();
    assert!(err.to_string().contains("401"), "{err}");
}

/// `DATABUG_TEST_ORACLE=host:port:service:user:password` (needs the
/// `oracle` feature and Oracle Instant Client). Not run in CI.
#[cfg(feature = "oracle")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oracle_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_ORACLE") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::Oracle);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].clone();
    cfg.user = p[3].clone();
    let mut conn = connect(&cfg, Some(&p[4])).await.unwrap();
    let _ = run(&mut conn, "DROP TABLE databug_orders", 10).await;
    run(&mut conn, "CREATE TABLE databug_orders (id NUMBER(10) PRIMARY KEY, status VARCHAR2(20), total NUMBER(12,2))", 10).await.unwrap();
    run(&mut conn, "INSERT INTO databug_orders SELECT LEVEL, CASE MOD(LEVEL, 3) WHEN 0 THEN 'pending' ELSE 'shipped' END, LEVEL + 0.5 FROM dual CONNECT BY LEVEL <= 200", 10).await.unwrap();
    let c = run(&mut conn, "SELECT * FROM databug_orders ORDER BY id LIMIT 5", 100).await.unwrap();
    assert_eq!(c.rows.rows(), 5);
    let cat = conn.introspect().await.unwrap();
    assert!(cat.find_table(None, "DATABUG_ORDERS").unwrap().column("ID").unwrap().primary_key);
    assert!(!conn.explain("SELECT * FROM databug_orders WHERE id = 3", false).await.unwrap().raw.is_empty());
    run(&mut conn, "DROP TABLE databug_orders", 10).await.unwrap();
}

/// `DATABUG_TEST_DB2=host:port:database:user:password` (needs the `db2`
/// feature, unixODBC and the IBM Db2 ODBC driver). Not run in CI.
#[cfg(feature = "db2")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db2_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_DB2") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::Db2);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].clone();
    cfg.user = p[3].clone();
    let mut conn = connect(&cfg, Some(&p[4])).await.unwrap();
    seed_store(&mut conn, Dialect::Postgres).await;
    store_checks(&mut conn).await;
}

/// `DATABUG_TEST_SPARK=host:port:database[:user]` for a Spark Thrift server
/// (needs the `spark` feature, unixODBC and a Spark ODBC driver). Not run in CI.
#[cfg(feature = "spark")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spark_end_to_end() {
    let Some(p) = env_parts("DATABUG_TEST_SPARK") else { return };
    let mut cfg = ConnectionConfig::new(DbKind::Spark);
    cfg.host = p[0].clone();
    cfg.port = p[1].parse().unwrap();
    cfg.database = p[2].clone();
    cfg.user = p.get(3).cloned().unwrap_or_default();
    let mut conn = connect(&cfg, None).await.unwrap();
    run(&mut conn, "CREATE OR REPLACE TEMPORARY VIEW nums AS SELECT id FROM range(100)", 10).await.unwrap();
    let c = run(&mut conn, "SELECT * FROM nums", 10).await.unwrap();
    assert!(c.truncated);
    let cat = conn.introspect().await.unwrap();
    assert!(!cat.schemas.is_empty());
}

/// `DATABUG_TEST_DUCKDB_DOWNLOAD=1` downloads DuckDB's library the way the
/// connection dialog does (into a temporary folder), then opens a database.
#[cfg(feature = "duckdb")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duckdb_download_and_load() {
    if std::env::var_os("DATABUG_TEST_DUCKDB_DOWNLOAD").is_none() || std::env::var_os("DATABUG_DUCKDB_LIB").is_some() {
        eprintln!("DATABUG_TEST_DUCKDB_DOWNLOAD not set (or DATABUG_DUCKDB_LIB is); skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("databug-runtimes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    databug_drivers::set_runtime_dir(dir.clone());
    let mut cfg = ConnectionConfig::new(DbKind::DuckDb);
    cfg.path = ":memory:".into();
    let err = connect(&cfg, None).await.err().unwrap();
    assert!(err.to_string().contains("Download DuckDB"), "{err}");
    assert!(!databug_drivers::duckdb_download().unwrap().installed);
    databug_drivers::start_duckdb_download(&tokio::runtime::Handle::current(), Arc::new(|| {}));
    let start = Instant::now();
    loop {
        let d = databug_drivers::duckdb_download().unwrap();
        if !d.state.running {
            assert!(d.state.error.is_none(), "{:?}", d.state.error);
            assert!(d.installed);
            assert_eq!(d.state.downloaded, d.size);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(300), "download took too long");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // only the library is kept
    let files: Vec<_> = std::fs::read_dir(dir.join("duckdb-1.5.6")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let mut conn = connect(&cfg, None).await.unwrap();
    let c = run(&mut conn, "SELECT version() AS v", 10).await.unwrap();
    assert_eq!(c.rows.cell(0, 0), Some("v1.5.6"));
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
