//! The UI-verification bridge is a development-only plugin, so its permission must not
//! be named in a build that does not include it — Tauri fails the build on an unknown
//! permission, which is how a release build first broke.
//!
//! The capability lives in `capabilities/dev-bridge.json.in` and is copied into place
//! only when the `ui-bridge` feature is on, then removed again when it is not.

use std::path::Path;

fn main() {
    let live = Path::new("capabilities/dev-bridge.json");
    let template = Path::new("capabilities/dev-bridge.json.in");

    if cfg!(feature = "ui-bridge") {
        if let Ok(body) = std::fs::read_to_string(template) {
            let _ = std::fs::write(live, body);
        }
    } else if live.exists() {
        let _ = std::fs::remove_file(live);
    }

    println!("cargo:rerun-if-changed=capabilities/dev-bridge.json.in");
    tauri_build::build()
}
