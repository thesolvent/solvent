//! Periodic intake, evaluation, expiry, and chain reconciliation for rebate batches.

use std::sync::Arc;

use alloy_primitives::{keccak256, U256};
use alloy_sol_types::SolValue;

use crate::deps::ledger::Clock;
use crate::deps::rebate::{RebateAccrualSource, RebateChainSource, RebateStore};
use crate::deps::registry::EventStore;
use crate::ledger::{AvailableSnapshot, LedgerService};
use crate::primitives::rebate::{RebateBatch, RebateBatchState, RebateMinimums};
use crate::primitives::registry::Snapshot;
use crate::primitives::{ChainId, ReservationId, SolventError};
use crate::registry::SharedSnapshot;

use super::{RebateEvaluationInput, RebateService};

const INTAKE_BATCH_LIMIT: u32 = 256;

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RebateWorkerConfig {
    pub chain: ChainId,
    pub start_block: u64,
    pub authorization_ttl_blocks: u64,
    pub reservation_ttl_secs: u64,
}

impl RebateWorkerConfig {
    pub fn new(
        chain: ChainId,
        start_block: u64,
        authorization_ttl_blocks: u64,
        reservation_ttl_secs: u64,
    ) -> Self {
        Self {
            chain,
            start_block,
            authorization_ttl_blocks,
            reservation_ttl_secs,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebateWorkerReport {
    pub accrued: usize,
    pub evaluated: usize,
    pub settled: usize,
    pub expired: usize,
}

pub struct RebateWorker {
    service: Arc<RebateService>,
    accruals: Arc<dyn RebateAccrualSource>,
    chain: Arc<dyn RebateChainSource>,
    store: Arc<dyn RebateStore>,
    registry_events: Arc<dyn EventStore>,
    registry: Arc<SharedSnapshot>,
    ledger: Arc<LedgerService>,
    clock: Arc<dyn Clock>,
    config: RebateWorkerConfig,
}

enum BatchOutcome {
    Unchanged,
    Evaluated,
    Expired,
}

impl RebateWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        service: Arc<RebateService>,
        accruals: Arc<dyn RebateAccrualSource>,
        chain: Arc<dyn RebateChainSource>,
        store: Arc<dyn RebateStore>,
        registry_events: Arc<dyn EventStore>,
        registry: Arc<SharedSnapshot>,
        ledger: Arc<LedgerService>,
        clock: Arc<dyn Clock>,
        config: RebateWorkerConfig,
    ) -> Self {
        Self {
            service,
            accruals,
            chain,
            store,
            registry_events,
            registry,
            ledger,
            clock,
            config,
        }
    }

    /// Process only blocks already folded into the registry snapshot, then evaluate complete batches.
    pub async fn tick(&self, current_head: u64) -> Result<RebateWorkerReport, SolventError> {
        let Some(registry_cursor) = self.registry_events.cursor(self.config.chain).await? else {
            return Ok(RebateWorkerReport::default());
        };
        let safe_head = current_head.min(registry_cursor.block_number);
        let mut report = RebateWorkerReport {
            settled: self.reconcile_executions(safe_head).await?,
            ..RebateWorkerReport::default()
        };

        loop {
            let pending = self.accruals.pending(INTAKE_BATCH_LIMIT).await?;
            let count = pending.len();
            for accrual in pending {
                self.service.accrue(accrual).await?;
                report.accrued += 1;
            }
            if count < INTAKE_BATCH_LIMIT as usize {
                break;
            }
        }

        let snapshot = self.registry.load();
        let caps = self.ledger.snapshot();
        // Independent strategies still make progress when one batch fails; supervision receives
        // the first error after the complete pass.
        let mut first_error = None;
        for batch in self.service.open_batches().await {
            match self
                .process_batch(&batch, safe_head, current_head, &snapshot, &caps)
                .await
            {
                Ok(BatchOutcome::Unchanged) => {}
                Ok(BatchOutcome::Evaluated) => report.evaluated += 1,
                Ok(BatchOutcome::Expired) => report.expired += 1,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(report)
    }

    async fn process_batch(
        &self,
        batch: &RebateBatch,
        safe_head: u64,
        current_head: u64,
        snapshot: &Snapshot,
        caps: &AvailableSnapshot,
    ) -> Result<BatchOutcome, SolventError> {
        if matches!(batch.state, RebateBatchState::Ready(_)) {
            return Ok(
                if self
                    .service
                    .expire_ready(&batch.strategy, safe_head)
                    .await?
                {
                    BatchOutcome::Expired
                } else {
                    BatchOutcome::Unchanged
                },
            );
        }
        if batch
            .accruals
            .values()
            .any(|accrual| accrual.confirmed_block > safe_head)
            || self.accruals.has_unsettled(&batch.strategy).await?
        {
            return Ok(BatchOutcome::Unchanged);
        }
        let Some(strategy) = snapshot.strategy(&batch.strategy) else {
            return Ok(BatchOutcome::Unchanged);
        };
        if !strategy.active {
            self.service.discard(&batch.strategy).await?;
            return Ok(BatchOutcome::Unchanged);
        }
        let minimums = [
            RebateMinimums::new(batch.pair.lo, U256::ONE, U256::ONE),
            RebateMinimums::new(batch.pair.hi, U256::ONE, U256::ONE),
        ];
        let deadline_block = current_head.saturating_add(self.config.authorization_ttl_blocks);
        let reservation_id = ReservationId(keccak256((batch.id.0, deadline_block).abi_encode()));
        self.service
            .evaluate(RebateEvaluationInput {
                strategy,
                caps,
                minimums: &minimums,
                reservation_id,
                reservation_ttl_secs: self.config.reservation_ttl_secs,
                deadline_block,
                published_at: self.clock.now_unix(),
            })
            .await?;
        Ok(BatchOutcome::Evaluated)
    }

    async fn reconcile_executions(&self, safe_head: u64) -> Result<usize, SolventError> {
        let cursor = self.store.scan_cursor(self.config.chain).await?;
        let from_block = cursor.map_or(self.config.start_block, |block| block.saturating_add(1));
        if from_block > safe_head {
            return Ok(0);
        }
        let events = self.chain.fetch(from_block, safe_head).await?;
        let now = self.clock.now_unix();
        let mut settled = 0;
        for event in events {
            settled += usize::from(self.service.settle(event, now).await?);
        }
        self.store
            .save_scan_cursor(self.config.chain, safe_head)
            .await?;
        Ok(settled)
    }
}
