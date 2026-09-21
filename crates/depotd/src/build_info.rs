pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = env!("DEPOT_BUILD_ID");

pub fn version_line(program: &str) -> String {
    format!("{program} {VERSION} (build {BUILD_ID})")
}
