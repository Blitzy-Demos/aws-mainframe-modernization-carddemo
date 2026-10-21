# Previous-release query baseline

Previous release: 59cc6c2 (empty query set)

## Purpose

This folder holds the sqlx query set of the release before this one. `crates/persistence/tests/explain.rs` plans every query in it with `EXPLAIN (GENERIC_PLAN)` against a database migrated from `crates/persistence/migrations/`, so a migration that removed or retyped something the previous release still queries fails the test. This is the expand-then-contract evidence of the Single Schema Executor rule.

## Why the set is empty

Commit `59cc6c2` is the repository state before `modern/` existed. It holds no PostgreSQL schema, no sqlx query and no `.sqlx/` metadata, so no previous-release query can conflict with this release's migrations. This release's six migrations, `0001_reference_and_master.sql` to `0006_db2_transaction_types.sql`, only create.

The Db2 DDL under `app/app-transaction-type-db2/ddl/`, such as `TRNTYPE.ddl` and `TRNTYCAT.ddl`, is mainframe source read in place as the parity oracle, not an earlier release of this schema; migration `0006` reproduces its effect.

## Procedure for every later release

1. At release time, copy that release's `modern/.sqlx/query-*.json` files into this folder.
2. Remove `BASELINE.md` in the same change, so the folder holds either the copies or this file, never both.
3. Never edit a copy by hand. The copies come only from `cargo sqlx prepare --workspace -- --all-targets`, run against a database migrated from `crates/persistence/migrations/` by `carddemo-migrate`. `.sqlx/` is derived from the migrations and is never hand-edited either.
4. The next release's `explain.rs` plans each copy's `"query"` text against that release's own migrated schema.

## Contract rule

A migration that removes or retypes a column, table or index lands only after the previous release's code no longer uses it, which this folder proves. Expand first, contract later.

## How `explain.rs` judges this folder

The previous-release case passes when the folder holds at least one `query-*.json`, each of which must plan, or this file with the declaration line above. It fails for an empty folder, or for this file without that line.

## Where the schema comes from

- `runtime::migrate` (`crates/runtime/src/migrate.rs`) is the only migration entrypoint, through its `apply` function. The operator binary `carddemo-migrate` (`crates/runtime/src/bin/carddemo-migrate.rs`) calls it, and so does the Postgres harness of `test-support` for each test container.
- `carddemo-api`, `carddemo-mq` and `carddemo-batch` never migrate. Started against an unmigrated database, they fail at their first query.
- `crates/persistence/migrations/` is the only source of schema shape; `.sqlx/` is derived from it.
