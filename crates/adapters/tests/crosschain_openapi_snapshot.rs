//! Guards the cross-chain OpenAPI contract served by the proxy. Regenerate after changing a
//! cross-chain route or DTO with:
//!   SOLVENT_UPDATE_CROSSCHAIN_OPENAPI=1 cargo test -p solvent-adapters --test crosschain_openapi_snapshot

use solvent_adapters::http::crosschain_proxy::CrossChainApiDoc;
use utoipa::OpenApi;

#[test]
fn crosschain_openapi_snapshot_is_current() {
    let mut current =
        serde_json::to_string_pretty(&CrossChainApiDoc::openapi()).expect("serialize openapi");
    current.push('\n');

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/cross-chain-openapi.json"
    );

    if std::env::var_os("SOLVENT_UPDATE_CROSSCHAIN_OPENAPI").is_some() {
        std::fs::write(path, &current).expect("write cross-chain openapi snapshot");
        return;
    }

    let committed = std::fs::read_to_string(path).expect(
        "read sdk/cross-chain-openapi.json — create it with SOLVENT_UPDATE_CROSSCHAIN_OPENAPI=1",
    );
    assert_eq!(
        current, committed,
        "OpenAPI drift: regenerate sdk/cross-chain-openapi.json with SOLVENT_UPDATE_CROSSCHAIN_OPENAPI=1",
    );
}
