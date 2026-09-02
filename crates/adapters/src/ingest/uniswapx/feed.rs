//! An in-process order feed that mints and signs its own UniswapX orders — the E2E and testnet-demo
//! source, standing in for the deprecated hosted Orders API. Orders are built once up front; the
//! feed then streams them.

use futures::stream::{BoxStream, StreamExt};

use solvent_core::deps::ingest::OrderFeed;
use solvent_core::primitives::ingest::RawOrder;

use super::builder::{OrderSpec, SignedOrderBuilder};

pub struct SelfHostedFeed {
    orders: Vec<RawOrder>,
}

impl SelfHostedFeed {
    /// Build and sign each spec once, ready to stream.
    pub fn new(
        builder: &SignedOrderBuilder,
        specs: &[OrderSpec],
        observed_at: u64,
    ) -> SelfHostedFeed {
        SelfHostedFeed {
            orders: specs
                .iter()
                .map(|s| builder.build(s, observed_at))
                .collect(),
        }
    }
}

impl OrderFeed for SelfHostedFeed {
    fn stream(&self) -> BoxStream<'static, RawOrder> {
        futures::stream::iter(self.orders.clone()).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, B256, U256};
    use alloy::signers::local::PrivateKeySigner;

    use solvent_core::deps::ingest::Normalizer;
    use solvent_core::primitives::ingest::{AmountCurve, Exclusivity, ProtocolId};
    use solvent_core::primitives::ChainId;

    use super::super::UniswapXV2Normalizer;

    fn signer(byte: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from([byte; 32])).expect("valid test key")
    }

    // A self-hosted order, streamed and then normalized, recovers the spec it was built from — the
    // builder and the normalizer are inverse.
    #[tokio::test]
    async fn self_hosted_order_round_trips_through_the_normalizer() {
        let permit2 = address!("000000000022D473030F116dDEE9F6B43aC78BA3");
        let builder = SignedOrderBuilder::new(permit2, 1, signer(0x11), signer(0x22));
        let spec = OrderSpec {
            reactor: address!("2222222222222222222222222222222222222222"),
            nonce: U256::from(1u64),
            deadline: 5000,
            input_token: address!("6666666666666666666666666666666666666666"),
            input_start: U256::from(500u64),
            input_end: U256::from(500u64),
            output_token: address!("7777777777777777777777777777777777777777"),
            output_start: U256::from(1000u64),
            output_end: U256::from(900u64),
            recipient: address!("8888888888888888888888888888888888888888"),
            decay_start: 1000,
            decay_end: 1100,
            exclusive_filler: address!("5555555555555555555555555555555555555555"),
        };

        let feed = SelfHostedFeed::new(&builder, std::slice::from_ref(&spec), 1234);
        let orders: Vec<RawOrder> = feed.stream().collect().await;
        assert_eq!(orders.len(), 1);

        let intent = UniswapXV2Normalizer
            .normalize(&orders[0])
            .expect("normalizes");
        assert_eq!(intent.protocol, ProtocolId::UniswapXV2);
        assert_eq!(intent.origin_chain, ChainId(1));
        assert_eq!(intent.settler, spec.reactor);
        assert_eq!(intent.deadline, spec.deadline);
        assert_eq!(intent.input.token, spec.input_token);
        assert_eq!(intent.input.curve, AmountCurve::scalar(spec.input_start));
        assert_eq!(intent.outputs[0].token, spec.output_token);
        assert_eq!(intent.outputs[0].recipient, spec.recipient);
        assert_eq!(
            intent.outputs[0].curve,
            AmountCurve::dutch(
                spec.output_start,
                spec.output_end,
                spec.decay_start,
                spec.decay_end
            )
        );
        assert_eq!(
            intent.exclusivity,
            Some(Exclusivity {
                filler: spec.exclusive_filler,
                ends_at: spec.decay_start,
            })
        );
        assert_eq!(intent.signature, orders[0].signature);
        assert_eq!(intent.observed_at, 1234);
    }
}
