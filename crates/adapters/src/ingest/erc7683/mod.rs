//! Same-chain ERC-7683 order decoding, Permit2 authentication, and Aqua fill construction.

mod codec;
mod fill;
mod normalizer;

pub use codec::Solvent7683Order;
pub use fill::Erc7683FillBuilder;
pub use normalizer::{Erc7683Normalizer, NormalizedErc7683};
