//! Build script for the `runtime` crate: rebuild when the migration SQL changes, so the
//! migrator in `src/migrate.rs` always embeds the current `../persistence/migrations`.

fn main() {
    println!("cargo:rerun-if-changed=../persistence/migrations");
}
