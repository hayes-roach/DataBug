# Contributing to DataBug

Thanks for helping. This guide covers how to get the code running, how work flows from a branch to a release, and what a good pull request looks like. Bug reports, feature ideas, docs fixes and code are all welcome.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security problems go through the [security policy](SECURITY.md), not public issues.

## Ways to help

- **Report a bug** with the bug report form under [Issues](https://github.com/hayes-roach/DataBug/issues/new/choose). Include the DataBug version, your OS, the database engine and version, and the steps to reproduce.
- **Suggest a feature** with the feature request form. Explain the problem you're trying to solve, not only the feature you have in mind.
- **Pick up an issue.** Issues labelled `good first issue` are small and well described; `help wanted` ones are larger. Comment on an issue before starting so two people don't build the same thing.
- **Test against real servers.** SQL Server and MongoDB haven't been run against live servers yet (see *Not done yet* in the README). Reports from real setups are valuable even without a fix.

For anything bigger than a bug fix (a new driver, a new panel, a change to how sessions work), open an issue or a draft pull request to agree on the approach first.

## Getting set up

You need Rust 1.81 or newer and a C compiler. Nothing else is required on macOS, Windows or Linux.

```sh
git clone https://github.com/hayes-roach/DataBug.git
cd DataBug
cargo run                     # debug build of the app
cargo test --workspace        # unit tests + SQLite end-to-end tests
```

To keep your test settings away from your real ones, point DataBug at a scratch folder:

```sh
DATABUG_HOME=/tmp/databug-dev DATABUG_NO_KEYRING=1 cargo run
```

### Live database tests

The Postgres and MySQL tests run when these variables are set (CI sets them):

```sh
DATABUG_TEST_PG=localhost:5432:store:postgres \
DATABUG_TEST_MYSQL=127.0.0.1:3306:store:root:pw \
cargo test --workspace
```

Seed each database with `crates/databug-app/assets/sample.sql` first; `.github/workflows/ci.yml` shows the exact commands. Docker is the quickest way to get a server:

```sh
docker run -d --name pg -p 5432:5432 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:16
docker run -d --name my -p 3306:3306 -e MYSQL_ROOT_PASSWORD=pw -e MYSQL_DATABASE=store mysql:8.4
```

### Checking UI changes

`databug --screenshot out.png` drives the UI without a person and saves a PNG. `DATABUG_SHOT_SCENE` picks a scene (`light`, `find`, `theme`, `multidb`, `plan`, `dialog` and more; see the README). Attach before/after screenshots to any pull request that changes how something looks, in both a dark and a light theme.

## Where things live

| Crate | What's in it |
|---|---|
| `databug-core` | Shared types: connection config, schema catalog, compact result sets, plans |
| `databug-sql` | SQL intelligence with no UI or driver dependencies: lexer, splitter, completion, inspections, formatter, DDL generation, schema diff, rename |
| `databug-drivers` | One async `DbConnection` trait with an implementation per engine, each behind a cargo feature |
| `databug-app` | The egui desktop app |

Keep `databug-sql` free of UI and driver dependencies so it can back a language server later. Engine-specific SQL belongs in the driver or in `databug-sql` behind a `Dialect` match, not in the app.

## Branching strategy

DataBug uses **GitHub flow**: one long-lived branch, `main`, plus short-lived branches for each change.

- **`main` is always releasable.** It builds and passes tests on Linux, macOS and Windows. Nobody pushes to it directly; every change lands through a pull request.
- **Branch from the latest `main`** and name the branch after the kind of change:

  | Prefix | For | Example |
  |---|---|---|
  | `feat/` | New features | `feat/ssh-tunnels` |
  | `fix/` | Bug fixes | `fix/mssql-default-database` |
  | `docs/` | Documentation only | `docs/kerberos-setup` |
  | `refactor/` | Code changes that don't change behaviour | `refactor/explorer-rows` |
  | `test/` | Tests only | `test/mysql-introspection` |
  | `ci/` | Workflows, packaging and installers | `ci/windows-arm64` |
  | `chore/` | Dependency bumps and housekeeping | `chore/bump-tokio` |

- **Keep branches short-lived.** Aim to merge within a few days. Bring a long-running branch up to date by rebasing on `main` (`git pull --rebase origin main`) rather than merging `main` into it.
- **One change per branch.** A bug fix and an unrelated refactor are two pull requests.
- **Pull requests are squash-merged**, so the pull request title becomes the commit on `main`. The branch is deleted after merging.
- **Forks work the same way.** Fork the repository, create a branch in your fork, and open a pull request against `main`.

### Releases and hotfixes

- Versions follow [Semantic Versioning](https://semver.org/). Before 1.0, a minor bump (`0.2.0`) may contain breaking changes to settings or behaviour; patch releases (`0.1.1`) only fix bugs.
- To release, a maintainer:
  1. Moves the *Unreleased* entries in [CHANGELOG.md](CHANGELOG.md) under the new version with today's date.
  2. Bumps `version` in the root `Cargo.toml`.
  3. Merges that as a pull request titled `chore(release): vX.Y.Z`.
  4. Tags the merge commit on `main` and pushes the tag: `git tag -a vX.Y.Z -m "DataBug X.Y.Z" && git push origin vX.Y.Z`.

  The tag triggers `.github/workflows/release.yml`, which builds the installers and publishes the GitHub release. Tags are never moved or deleted once a release is published.
- **Hotfixes** normally go to `main` like any fix, followed by a patch release. If `main` already holds unreleased work that shouldn't ship yet, a maintainer creates `release/X.Y` from the last tag, merges the fix there through a pull request, tags `vX.Y.Z+1` from that branch, and brings the fix back to `main` with a pull request.

## Commit messages and pull request titles

Use [Conventional Commits](https://www.conventionalcommits.org/) for pull request titles (and for commits, where you can):

```
<type>(<optional scope>): <summary in the imperative, under 72 characters>
```

- **Types:** `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `ci`, `chore`
- **Scopes:** the area changed, e.g. `explorer`, `editor`, `grid`, `sql`, `pg`, `mysql`, `mssql`, `sqlite`, `mongo`, `installer`
- Mark breaking changes with `!` and explain them in the body: `feat(settings)!: store themes per profile`

Examples:

```
feat(mssql): list procedure parameters in the explorer
fix(editor): keep find highlights in sync after replace all
docs: explain Kerberos setup on Linux
```

The body says *why* the change is needed and anything a reviewer should know. Reference issues with `Fixes #123` so they close when the pull request merges.

## Code guidelines

- **Match the style around you.** The code isn't run through `cargo fmt` (its lines are longer than rustfmt's defaults), so don't reformat files you're not otherwise changing. Keep diffs focused on the change.
- **The UI never blocks.** Database I/O runs on the Tokio runtime inside sessions; the UI talks to them over channels. Don't call `block_on` or do network or disk work that can take time on the UI thread.
- **Repaint only when needed.** DataBug uses no CPU while idle. Request repaints from events, not on a timer, unless something is animating.
- **Memory matters.** Results live in compact arenas (`ResultSet`). Don't copy whole result sets or store a `String` per cell.
- **Use the palette.** Take colors from `Palette` (`crate::theme`) so all six themes and custom colors work. Never hard-code a color for text or backgrounds.
- **Secrets stay in the keychain.** Passwords go through `Store::set_password`; never write them to `connections.json`, logs, history or catalog caches.
- **Guard rails stay on.** Changes that run SQL must keep the read-only, production and dangerous-statement checks (`check_and_dispatch`) in the path.
- **New drivers** implement `DbConnection`, sit behind a cargo feature in `databug-drivers`, and fill in `Capabilities` honestly so the UI can adapt.
- **Tests:** add unit tests for logic in `databug-sql` and `databug-core`, and live tests in `crates/databug-drivers/tests/live.rs` for driver behaviour. Every bug fix should come with a test that fails without the fix, where that's practical.
- **Docs:** update the README when you add or change a user-facing feature or shortcut, and add a line under *Unreleased* in [CHANGELOG.md](CHANGELOG.md).

## Pull request checklist

Before you ask for a review:

- [ ] The branch is up to date with `main`
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release` builds without new warnings
- [ ] UI changes include screenshots in dark and light themes
- [ ] README and CHANGELOG are updated where it matters
- [ ] The title follows Conventional Commits

CI runs the tests against live Postgres 16 and MySQL 8.4 on Linux, and builds and tests on macOS and Windows. All checks must pass, and a maintainer must approve, before a pull request can merge. Draft pull requests are welcome for early feedback.

## Reviews

Reviewers aim to respond within a few days. Feedback is about the code, not the person. If you disagree with a suggestion, say why; the goal is the best result, not winning. Mark conversations resolved once they're addressed, and push follow-up commits rather than force-pushing during review, so reviewers can see what changed.

## License

DataBug is licensed under [Apache-2.0](LICENSE). By contributing, you agree that your contributions are licensed under the same terms (section 5 of the license). You keep the copyright to your work; no separate contributor agreement is needed.
