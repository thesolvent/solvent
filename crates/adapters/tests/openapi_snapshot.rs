//! Guards that the OpenAPI snapshot the SDK generates its types from cannot drift from the
//! handlers. Regenerate after changing any route/DTO with:
//!   SOLVENT_UPDATE_OPENAPI=1 cargo test -p solvent-adapters --test openapi_snapshot

use solvent_adapters::http::openapi::ApiDoc;
use utoipa::OpenApi;

#[test]
fn openapi_snapshot_is_current() {
    let mut current = serde_json::to_string_pretty(&ApiDoc::openapi()).expect("serialize openapi");
    current.push('\n');

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/openapi.json");

    if std::env::var_os("SOLVENT_UPDATE_OPENAPI").is_some() {
        std::fs::write(path, &current).expect("write openapi snapshot");
        return;
    }

    let committed = std::fs::read_to_string(path)
        .expect("read sdk/openapi.json — create it with SOLVENT_UPDATE_OPENAPI=1");
    assert_eq!(
        current, committed,
        "OpenAPI drift: regenerate sdk/openapi.json with SOLVENT_UPDATE_OPENAPI=1",
    );
}
