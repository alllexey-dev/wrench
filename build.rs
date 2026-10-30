// The version string embeds these at build time (see `wrench::version`).
fn main() {
    for var in ["VERSION_SUFFIX", "GIT_HASH", "GIT_DATE"] {
        println!("cargo:rerun-if-env-changed={}", var);
    }
}
