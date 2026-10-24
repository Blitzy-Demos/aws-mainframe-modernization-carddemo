# Previous-release query baseline

Previous release: 59cc6c2 (empty query set)

## Purpose

This folder holds the sqlx query set of the release immediately before this one, and only that release's set. `crates/persistence/tests/explain.rs` plans every query in it with `EXPLAIN (GENERIC_PLAN)` against a database migrated from `crates/persistence/migrations/`. The test fails when a migration removes a table, column, type or function that a previous-release query names, or retypes a column so that the query no longer type-checks. This plannability check is the previous-release query test of the Single Schema Executor rule, not a complete compatibility proof.

## Why the set is empty

Commit `59cc6c2` is the repository state before `modern/` existed. It holds no PostgreSQL schema, no sqlx query and no `.sqlx/` metadata, so no previous-release query can conflict with this release's migrations. This release's six migrations, `0001_reference_and_master.sql` to `0006_db2_transaction_types.sql`, only create.

The Db2 DDL under `app/app-transaction-type-db2/ddl/`, such as `TRNTYPE.ddl` and `TRNTYCAT.ddl`, is mainframe source read in place as the parity oracle, not an earlier release of this schema; migration `0006` reproduces its effect.

## Procedure for every later release

The snapshot is replaced, never added to: the folder holds one release's query set.

1. Make the first change after a release while `modern/.sqlx/` still equals the shipped release's: from `modern/`, `git diff --quiet <release> -- .sqlx` exits 0 and `git status --porcelain -- .sqlx` prints nothing. In that one change, replace the folder's contents from `modern/`:

   ```sh
   git rm -q --ignore-unmatch 'crates/persistence/compat/previous/*'
   mkdir -p crates/persistence/compat/previous
   cp .sqlx/query-*.json crates/persistence/compat/previous/
   git add crates/persistence/compat/previous
   ```

   The `git rm` removes the earlier snapshot's copies and `BASELINE.md` alike, so the folder holds either the copies or this file, never both. The `git add` stages the new copies into the same change.
2. Check membership from `modern/`: `diff -r .sqlx crates/persistence/compat/previous` prints nothing, so the folder holds exactly the shipped release's files, byte for byte. It also reports any leftover untracked file, which `git rm` leaves in place and `git add` would stage.
3. Never edit a copy by hand. The copies come only from `cargo sqlx prepare --workspace -- --all-targets`, run against a database migrated from `crates/persistence/migrations/` by `carddemo-migrate`. `.sqlx/` is derived from the migrations and is never hand-edited either.
4. The next release's `explain.rs` plans each copy's `"query"` text against that release's own migrated schema.

## What the check does not cover

`explain.rs` does not compare the result and parameter types recorded in each copy's `"describe"` with the migrated schema, and it does not inspect the plan's access path. A retyped column can still plan while the previous release's code fails to decode it. Removing an index that served a query leaves the query plannable, as a sequential scan. A contract migration that retypes a column or removes an index therefore also needs a review of every copy that names it: its recorded types against the new schema, and its access path. `cargo sqlx prepare --workspace --check` and the index assertion of `explain.rs` cover only this release's own queries.

## Contract rule

A migration that removes or retypes a column, table or index lands only after the previous release's code no longer uses it. The plannability check enforces this for the objects each previous-release query names, and the review above does so for types and indexes. Expand first, contract later.

## How `explain.rs` judges this folder

The previous-release case passes when the folder holds at least one `query-*.json`, each of which must plan, or this file with the declaration line above. It fails for an empty folder, or for this file without that line. The test does not check which release the copies come from; the membership check of the procedure does.

## Where the schema comes from

- `runtime::migrate` (`crates/runtime/src/migrate.rs`) is the only migration entrypoint, through its `apply` function. The operator binary `carddemo-migrate` (`crates/runtime/src/bin/carddemo-migrate.rs`) calls it, and so does the Postgres harness of `test-support` for each test container.
- `carddemo-api`, `carddemo-mq` and `carddemo-batch` never migrate. Started against an unmigrated database, they fail at their first query.
- `crates/persistence/migrations/` is the only source of schema shape; `.sqlx/` is derived from it.
