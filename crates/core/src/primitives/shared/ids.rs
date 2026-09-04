//! Typed identifiers. Each wraps a fixed-bytes value in its own type, so passing the wrong id to
//! the wrong function is a compile error, not a runtime mix-up. DB column: `bytea`.

use alloy_primitives::{Address, B256};

/// Define a newtype id over a hex-parsable inner type (`B256`, `Address`), with `Display`/`FromStr`.
macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub $inner);

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&self.0, f)
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = $crate::SolventError;
            fn from_str(s: &str) -> ::core::result::Result<Self, Self::Err> {
                s.parse::<$inner>().map(Self).map_err(|e| $crate::SolventError::InvalidId {
                    id_type: stringify!($name),
                    reason: e.to_string(),
                })
            }
        }
    };
}

define_id!(
    /// UniswapX order hash.
    IntentId(B256)
);
define_id!(
    /// Maker (liquidity provider) wallet address.
    MakerId(Address)
);
define_id!(
    /// keccak256 of a shipped Aqua strategy's program bytes.
    StrategyHash(B256)
);
define_id!(
    /// Off-chain reservation identifier.
    ReservationId(B256)
);
define_id!(
    /// TTL lease identifier.
    LeaseId(B256)
);
define_id!(
    /// On-chain fill identifier.
    FillId(B256)
);

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::FromStr;

    #[test]
    fn ids_round_trip_hex_and_reject_garbage() {
        let h = "0x1111111111111111111111111111111111111111111111111111111111111111";
        let id = IntentId::from_str(h).unwrap();
        assert_eq!(id.to_string(), h);
        assert_eq!(IntentId::from_str(&id.to_string()).unwrap(), id);

        let a = "0x00000000000000000000000000000000000000aa";
        let maker = MakerId::from_str(a).unwrap();
        assert_eq!(MakerId::from_str(&maker.to_string()).unwrap(), maker);

        assert!(IntentId::from_str("not-hex").is_err());
    }
}
