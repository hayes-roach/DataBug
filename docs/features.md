# Features

Everything DataBug does, in detail. Back to the [README](../README.md).

**Schema visibility**
- Under each connection, a **"3 of 12 schemas"** chip opens a checklist with **All**, **Default only** and **Non-empty** shortcuts and a filter for long lists. Also in the connection's right-click menu (**Choose schemas…**) and in the connection dialog.
- The choice is saved per connection. Ticking every schema switches back to "All", which also picks up schemas created later. MySQL calls them databases.
- Hidden schemas are only hidden in the explorer; completion, compare and the ER diagram still see everything.

**Databases** (several per server, like schemas)
- PostgreSQL, SQL Server and MongoDB connections get a **"2 of 9 databases"** chip (also *Choose databases…* in the connection's menu). Tick databases to show each as its own node with its own schemas, schema picker, consoles and completion. *All* and *Default only* shortcuts.
- When a connection names no database, DataBug uses the default: `postgres` on PostgreSQL, the login's default database on SQL Server, `test` on MongoDB.

**Connections and projects**
- *Disconnect* (right-click a connection) closes its sessions but keeps it in the explorer; *Connect*, a double-click, or running a query reconnects. A green dot at the right of a connection means it's signed in right now (amber while it signs in). The small dot at the bottom right of its icon is solid green while connected and a grey ring when it isn't; the dots in the toolbar and on its console tabs show the connection's color, filled while connected and a hollow ring when not. A connection the server drops (restart, network loss, timeout) shows as disconnected and signs in again on the next query.
- Drag connections to reorder them.
- The refresh button in the explorer header (left of Find) reloads the schema of the current database: the open console's, or the connection or database selected in the explorer. It counts as asking to sign in again, so it also retries a connection whose sign-in failed.
- **No automatic retries after a failed sign-in.** When a connection can't sign in, DataBug shows the error and stops: no reconnecting in the background, no second attempt for queued work, so a wrong password can't lock the account. *Retry*, *Connect*, *Refresh schema*, running a query or editing the connection tries again.
- **Projects**: the switcher above the tree picks *All connections* or a project. *New project…* and *Rename project…* open a small card with the name field ready to type in (Enter creates or saves, Esc or *Cancel* closes it). Each project keeps its own ordered list of connections; new connections join the current project; right-click → *Add to project* / *Remove from project*. Saved in `projects.json`.

**Running procedures**
- Right-click a procedure → *Execute (EXEC)…* on SQL Server or *Execute (CALL)…* on PostgreSQL/MySQL; functions get *Run in console…* (`SELECT …`). Each parameter gets a `NULL` placeholder with its declared type as a comment; routines without parameters run immediately, and only their results are shown (the SQL isn't added to your console).
- Completion after `EXEC`, `EXECUTE` and `CALL` lists procedures with their parameters.

**Find**
- `Ctrl/⌘ + F` in the editor opens a find bar like VS Code's: match case (`Alt+C`), whole word (`Alt+W`), regex (`Alt+R`), "2 of 14", Enter / Shift+Enter or F3 / ⌘G to step, all matches highlighted. `Ctrl + H` (⌘⌥F on macOS) adds replace (one or all, `$1` groups with regex).
- **Find in database**: press `Ctrl/⌘ + F` again in the find bar, outside the editor, or use *Find* in the explorer. Searches object names, columns, and optionally the definitions of views, routines and triggers, in one connection or all of them. Click a result to reveal it in the explorer, double-click to open it. *Search table data…* generates a query that counts matching rows in every text column.

**Themes**
- Eight built-in themes: Dark (default), Light, Midnight, Forest, Pink, Purple, Paper and High contrast, picked from the toolbar's theme menu. *Customize colors…* lets you change any of the 25 interface and syntax colors per theme, with per-color and whole-theme reset.

**Editor**
- Syntax highlighting per dialect, line numbers, current-line highlight, toggle comment (`Ctrl/⌘ + /`), formatter (`Ctrl + Alt + L`).
- Schema-aware completion that works on half-typed SQL: columns resolved through aliases (including aliases defined *after* the cursor), tables after `FROM`/`JOIN`, `schema.` drill-down, fuzzy matching (`cid` → `customer_id`), and join conditions after `ON` inferred from `<table>_id` naming.
- Inspections as you type: unknown tables, unknown `alias.column` references, `DELETE`/`UPDATE` without `WHERE`, `DROP`/`TRUNCATE`.
- **Run** (in the bar above the editor, or `Ctrl/⌘ + Shift + Enter`) runs the selected code, or the whole console when nothing is selected. `Ctrl/⌘ + Enter` runs just the statement at the cursor. The running statement is highlighted, and a failing statement is tinted red.
- On SQL Server, Run sends each batch (the text between `GO` lines) as one request, as SQL Server Management Studio does, so `DECLARE`d variables, temp tables and `SET` options apply to all of it; `Ctrl/⌘ + Enter` still runs one statement, or the whole batch when it declares variables or creates a procedure, function, trigger or view. On Oracle and Db2, `DECLARE … BEGIN … END;` and `BEGIN … END;` blocks, procedures, functions and triggers, and packages up to a `/` line. PostgreSQL's `BEGIN;` transactions, `DECLARE … CURSOR FOR` and `$$` function bodies split as before.
- Put the cursor on a name (a table, column, alias or variable) and every other use of it in the console is softly highlighted; strings, comments and keywords that happen to match aren't.
- Right-click in the editor for *Cut*, *Copy*, *Paste*, *Select all*, *Run all* / *Run selection*, *Run statement at cursor*, *Go to definition*, *Find…*, *Format* and *Toggle comment*. Dragging a selection to the top or bottom edge keeps scrolling, so long selections follow the pointer.
- Output messages are selectable text (drag across them and press `Ctrl/⌘ + C`); right-click a line for *Copy message*, *Copy SQL*, *Copy line* or *Copy all output*.
- **SQL Server batches**: a batch (the text between `GO` lines) that declares variables (`DECLARE @id int; SET @id = 5; EXEC dbo.load @id = @id;`) runs as one request, so the variables reach every statement; so does a `CREATE`/`ALTER PROCEDURE`, `FUNCTION`, `TRIGGER` or `VIEW` batch, whose body is the rest of the batch. Other scripts still run statement by statement, and read-only / production checks still look at each statement in the batch.
- **Go to definition**: `Ctrl/⌘ + B`, `Ctrl/⌘ + click`, or `Control + F` on macOS (where Find is `⌘ + F`) on the name of a procedure, function, trigger or view opens its source in a new console; on a table it opens the table's DDL. Unqualified names are looked up in the console's schema first, then the others. If it's already open, DataBug switches to that tab.
- **Each console has its own database and schema**: its tab shows them in brackets (`console_2 [warehouse / sales]`), and the schema picker at the right of the console's toolbar (`main ▾`) switches them. Picking a schema makes it the one unqualified names resolve to in completion, inspections and go to definition, and in the session: `SET search_path` on PostgreSQL, CockroachDB and TimescaleDB, `USE` on MySQL and Cassandra, `CURRENT_SCHEMA` on Oracle, `SET SCHEMA` on Db2, H2 and DuckDB. SQL Server keeps resolving names in queries with the login's default schema, so qualify them there. With several databases shown for a connection, the picker also moves the console to another database.

**Tabs and consoles**
- Right-click a tab: *Close*, *Close other tabs*, *Close tabs to the left*, *Close tabs to the right*, *Close all tabs*. Middle-click closes a tab.
- Consoles you don't close are kept, with their text, connection and schema, and survive restarts (`editors.json` in the data folder). The **Consoles** panel under the database explorer lists them by connection: the open tabs (with the time of their last run; click to switch) and the consoles still open when DataBug last quit (dimmed; click to reopen, right-click to delete). New consoles are named `console`, `console_2`, … like DataGrip's. Closing a tab removes its console from the list; `Ctrl/⌘ + Shift + T` brings back the last one closed during this run. Empty consoles aren't kept, nor are consoles of a deleted connection.

**Results**
- Virtualized grid (only visible rows are laid out), with column types in the header, click-to-sort (numeric and natural ordering), row filter, keyboard navigation, and copy cell/row.
- Row editing for single-table queries that include the primary key: double-click a cell (or F2) to edit it, click row numbers to select whole rows (⌘/Ctrl-click and Shift-click for several), *− Delete* or the Delete key to remove them, *+ Row* (Alt + Insert) to add one, *Duplicate* to copy one. Added, changed and deleted rows are color-coded and nothing is saved until *Submit*, which runs `DELETE`, `UPDATE` and `INSERT` statements keyed on the primary key (*Preview SQL* copies them, *Revert* undoes everything). If the database refuses a change, the pending edits stay so you can fix them.
- Streams rows in batches with a configurable row limit (default 10,000). Postgres cancels the rest of an over-limit result on the server instead of draining it.
- Export to CSV, JSON, SQL `INSERT`s or Markdown, or copy as TSV/CSV.
- Multiple result tabs per run, an Output log with timings, messages (Postgres `NOTICE`, MySQL warnings) and errors, and a searchable query history.

**Schema tools**
- **Modify table / New table**: edit columns (name, type, not null, primary key, default), indexes and foreign keys, with a live preview of the exact DDL for the engine. *Execute* runs it and reloads the schema; *Open in console* lets you review first. SQLite changes it can't make in place (types, keys, constraints) are done with a safe rebuild: create, copy, swap.
- **Schema compare**: pick any two connection + schema pairs. Lists what's only in the source, only in the target, or different (columns, types, nullability, defaults, keys, indexes, foreign keys, and optionally the text of views and routines). Each difference shows its DDL. Changes that can lose data are flagged and start unchecked. *Open migration in console* or *Run on target*.
- **ER diagrams**: tables and foreign keys with crow's-foot edges, auto-arranged by relationship, pan/zoom/drag, *Copy as Mermaid*, double-click a table to modify it.
- **CSV import**: into an existing table (columns matched by name, or mapped by hand) or a new table (column names cleaned up, types inferred). Delimiter auto-detected; runs in one transaction.
- **Rename refactoring** (`Shift + F6` on an identifier, or *Rename…* in the explorer): aliases are renamed throughout the statement; tables and columns are renamed in the console, and DataBug generates the `ALTER`/`sp_rename` plus rewritten definitions of every view, procedure and function that references them, for you to review and run.

**Plans**
- Explain and Explain Analyze render as a tree with time/cost bars. Selecting a node shows its details. *Copy original plan* gives you the plan in the engine's native JSON/XML/text. `EXPLAIN ANALYZE` of a write runs inside a transaction that's rolled back.

**Safety**
- Per-connection color, **Production** flag (red marker and a confirmation before anything that changes data), and **Read-only** flag (blocked in the client, and set at session level on Postgres, MySQL and SQLite).
- Confirmation for `DELETE`/`UPDATE` without `WHERE`, `DROP` and `TRUNCATE`.
- Manual transaction mode with an "open transaction" indicator and Commit/Rollback buttons.
- Cancel asks the server to stop the query. Pressing it again (or on engines without server cancel) drops and reopens the connection.

**Explorer**
- Connections → schemas → object groups → objects, with columns, indexes, foreign keys and triggers under each table.
- **Double-click a table or view** to see its first 100 rows. The query runs in the background and only the results appear, so your console isn't filled with `SELECT … LIMIT 100` lines. Double-clicking a procedure, function or trigger opens its definition; expand a table with its arrow to see its columns.
- **Filter objects** above the tree narrows every connection to matching tables, views, routines and other objects as you type; the × button or Esc clears it.
- The selected row gets a darker background with an accent bar.
- Engine logos ship with DataBug: they live in `crates/databug-app/assets/logos`, are compiled into the binary, and every release download includes them in a `logos` folder next to the application (next to `DataBug.exe` / `databug`; on macOS next to `DataBug.app`, and inside it). To swap one, replace `postgresql`, `mysql`, `sqlserver`, `sqlite`, `mongodb`, `couchdb`, `oracle`, `db2`, `duckdb`, `cassandra`, `cockroachdb`, `timescaledb`, `questdb`, `influxdb`, `spark` or `h2` (`.svg` or `.png`) in that folder and restart DataBug. Every engine has its logo, from the [Devicon](https://github.com/devicons/devicon) (MIT) and [Simple Icons](https://github.com/simple-icons/simple-icons) (CC0) sets or the project's own Apache-2.0 repository; [`SOURCES.md`](../crates/databug-app/assets/logos/SOURCES.md) lists where each file comes from and its license. `mariadb.png` is included for later (MariaDB detection).

**Connections**
- Passwords go to the OS keychain (macOS Keychain, Windows Credential Manager, Secret Service on Linux), never into config files. If no keychain is available, they're kept in memory for the session.
- `connections.json` and `settings.json` are plain JSON with no secrets, so they can live in Git.
- TLS for Postgres (`prefer`/`require`, encrypt without verifying, like libpq), MySQL (`require`) and SQL Server (with an optional *trust server certificate*).
- **SSH tunnels**: tick *Connect through an SSH server* in the connection dialog to reach a database behind a bastion or jump host, like `ssh -L`. Sign in with a password, a private key file (with its passphrase, if any) or the keys in your SSH agent (ssh-agent, 1Password, Pageant or the Windows OpenSSH agent). The database host and port are looked up from the SSH server, so `localhost` means the database runs on the SSH server itself. The SSH password or passphrase goes to the keychain like the database password. Host keys are checked against `~/.ssh/known_hosts`; a server found nowhere is trusted on first use and remembered in DataBug's own `known_hosts` file (next to `connections.json`), and a server whose key changed is refused. Sessions to the same database share one tunnel. Every network database works through a tunnel; SQLite and DuckDB files don't need one, and a MongoDB connection string (`mongodb+srv://…`) can't use one, so enter its host and port. TLS through a tunnel connects to `127.0.0.1`, so certificate checks that verify the host name fail: use *Prefer* or *Trust server certificate*.

**Performance**
- Result cells live in one string arena per result with a `u32` offset and a null bit per cell. That's about 4 bytes of overhead per cell instead of a heap-allocated `String` each.
- The UI repaints only when something changes, so CPU use is zero while idle. All database I/O runs on a 2-thread Tokio runtime and never blocks the UI.
- The schema catalog is cached on disk and refreshed in the background, so completion works immediately on startup.
- Low input latency: a frame takes about 1–2 ms, and each one is shown as soon as it's drawn (on Windows without waiting for vsync, using mailbox presentation where the GPU supports it), so hover highlights and scrolling keep up with fast mice and touchpads. Scrolling is immediate rather than animated.
- If DataBug still feels slow on a Windows machine, try another graphics backend: set `WGPU_BACKEND=vulkan` (or `opengl`) before starting it. `DATABUG_FRAME_STATS=1` prints how long frames take, every 120 frames, to the terminal it was started from (macOS, Linux, or a Windows debug build; Windows release builds have no console).

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl/⌘ + Enter` | Run statement at cursor (or selection) |
| `Ctrl/⌘ + Shift + Enter` (or the Run button) | Run the selection, or the whole console |
| `Ctrl/⌘ + Shift + E` | Explain plan |
| `Ctrl + Space` | Completion |
| `Ctrl + Alt + L` or `Ctrl/⌘ + Shift + F` | Format |
| `Ctrl/⌘ + /` | Toggle comment |
| `Ctrl/⌘ + F` | Find in the editor; again (or outside the editor) to find in the database |
| `Ctrl + H` / `⌘ + ⌥ + F` | Find and replace |
| `Enter` / `Shift + Enter`, `F3` / `Shift + F3` | Next / previous match |
| `Shift + F6` | Rename the table, column or alias under the cursor |
| `Ctrl/⌘ + T` / `Ctrl/⌘ + W` | New console / close console |
| `Ctrl/⌘ + Shift + T` | Reopen the last closed console |
| `Ctrl/⌘ + B`, `Ctrl/⌘ + click`, `Control + F` (macOS) | Open the definition of the procedure, function, view or table under the cursor |
| Arrows, `F2`, `Ctrl/⌘ + C` in the grid | Move, edit, copy cell |
| `Ctrl/⌘ + +` / `Ctrl/⌘ + −` / `Ctrl/⌘ + 0` | Larger / smaller / reset text size everywhere (also *Settings → Text size*, 70–200%, remembered) |
