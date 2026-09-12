use alloy::primitives::{Address, U256};
use solvent_adapters::ingest::uniswapx::{
    SimulatedBatchPool, SimulatedQuoteEngine, SimulatedQuoteRequest,
};

#[test]
fn claiming_a_batch_keeps_eight_prewarmed_virtual_makers_ready() {
    let batches = SimulatedBatchPool::seeded(7, 2);

    assert_eq!(batches.ready_batches(), 2);
    let first = batches.claim();
    let second = batches.claim();

    assert_eq!(first.maker_count(), 8);
    assert_eq!(second.maker_count(), 8);
    assert_ne!(first.id(), second.id());
    assert_eq!(batches.ready_batches(), 2);
}

#[test]
fn quoting_a_batch_is_in_memory_and_evaluates_every_virtual_maker() {
    let batches = SimulatedBatchPool::seeded(7, 1);
    let batch = batches.claim();
    let engine = SimulatedQuoteEngine::new();

    let quote = engine
        .quote(
            &batch,
            SimulatedQuoteRequest {
                token_in: Address::repeat_byte(0x11),
                token_out: Address::repeat_byte(0x22),
                amount_in: U256::from(1_000_000u64),
                input_decimals: 6,
                output_decimals: 18,
                market_out_per_in_q18: U256::from(2_000_000_000_000_000_000u128),
            },
        )
        .expect("the simulated book quotes");

    let reverse_quote = engine
        .quote(
            &batch,
            SimulatedQuoteRequest {
                token_in: Address::repeat_byte(0x22),
                token_out: Address::repeat_byte(0x11),
                amount_in: U256::from(2_000_000_000_000_000_000u128),
                input_decimals: 18,
                output_decimals: 6,
                market_out_per_in_q18: U256::from(500_000_000_000_000_000u128),
            },
        )
        .expect("the reverse simulated book quotes");

    assert_eq!(quote.makers_evaluated, 8);
    assert_eq!(reverse_quote.makers_evaluated, 8);
    assert!(
        quote.amount_out > U256::from(1_000_000_000_000_000_000u128),
        "expected a decimal-normalized WETH amount, got {}",
        quote.amount_out
    );
    assert!(
        reverse_quote.amount_out > U256::from(500_000u64),
        "expected a decimal-normalized USDC amount, got {}",
        reverse_quote.amount_out
    );
}
