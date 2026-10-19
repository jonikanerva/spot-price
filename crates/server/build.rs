//! Rebuild the server when a file in `migrations/` changes. `sqlx::migrate!`
//! embeds the migrations at compile time, but Cargo does not see a new file
//! in that directory without this build script.

fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
