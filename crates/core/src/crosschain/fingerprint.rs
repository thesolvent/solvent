use alloy_primitives::keccak256;

use crate::primitives::crosschain::LegQuote;

/// Hashes every authoritative leg field so a client cannot alter capital sources after quoting.
pub fn leg_quote_id(quote: &LegQuote) -> alloy_primitives::B256 {
    let mut bytes = Vec::with_capacity(320 + quote.sources.len() * 104);
    bytes.extend_from_slice(quote.request_id.as_slice());
    bytes.push(quote.role as u8);
    bytes.extend_from_slice(&quote.local_chain.0.to_be_bytes());
    bytes.extend_from_slice(&quote.remote_chain.0.to_be_bytes());
    bytes.extend_from_slice(quote.input_token.as_slice());
    bytes.extend_from_slice(quote.output_token.as_slice());
    bytes.extend_from_slice(&quote.amount_in.to_be_bytes::<32>());
    bytes.extend_from_slice(&quote.amount_out.to_be_bytes::<32>());
    bytes.push(quote.route as u8);
    bytes.extend_from_slice(&quote.block_number.to_be_bytes());
    bytes.extend_from_slice(&quote.expires_at_unix.to_be_bytes());
    bytes.extend_from_slice(&(quote.sources.len() as u64).to_be_bytes());
    for source in &quote.sources {
        bytes.extend_from_slice(source.maker.0.as_slice());
        bytes.extend_from_slice(source.strategy_hash.0.as_slice());
        bytes.extend_from_slice(source.token.as_slice());
        bytes.extend_from_slice(&source.amount.to_be_bytes::<32>());
    }
    keccak256(bytes)
}
