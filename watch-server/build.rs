// New or changed migrations are compiled into the binary (refinery's
// `embed_migrations!`): rebuild when they change.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
