import type { Address, Hex } from "viem";

export type CrossChainRoute = "direct" | "cctp";
export type LegRole = "origin" | "destination";
export type RemoteCommand =
    | "deliver"
    | "dispatch_fill_proof"
    | "claim_origin"
    | "dispatch_repayment"
    | "close_destination";

export interface LegQuote {
    quote_id: Hex;
    request_id: Hex;
    role: LegRole;
    local_chain: number;
    remote_chain: number;
    input_token: Address;
    output_token: Address;
    amount_in: Hex;
    amount_out: Hex;
    route: CrossChainRoute;
    block_number: number;
    expires_at_unix: number;
    sources: ReservationSource[];
}

export interface ReservationSource {
    maker: Address;
    strategy_hash: Hex;
    token: Address;
    amount: Hex;
}

export interface AggregateQuote {
    id: Hex;
    origin: LegQuote;
    destination: LegQuote;
    amount_in: Hex;
    amount_out: Hex;
    bridge_fee: Hex;
    cctp_finality_threshold?: number;
    expires_at_unix: number;
}

export interface PreparedStep {
    command: RemoteCommand;
    target: Address;
    value: Hex;
    calldata: Hex;
}

export interface ChainExecutionPlan {
    aggregate_id: Hex;
    chain_id: number;
    steps: PreparedStep[];
}

export interface CrossChainQuoteRequest {
    request_id: Hex;
    origin_chain_id: number;
    destination_chain_id: number;
    origin_token_in: Address;
    origin_token_out: Address;
    destination_token_in: Address;
    destination_token_out: Address;
    amount_in: Hex;
    destination_amount_in: Hex;
    deadline_unix: number;
    route: CrossChainRoute;
}

export interface CreateCrossChainOrderRequest {
    order_id: Hex;
    quote: AggregateQuote;
    origin_plan: ChainExecutionPlan;
    destination_plan: ChainExecutionPlan;
}

export interface DirectOrderDraftRequest {
    quote: AggregateQuote;
    sponsor: Address;
    recipient: Address;
    order_nonce: Hex;
    compact_nonce: Hex;
    compact_expires_unix: number;
}

export interface SolventCompactOrder {
    user: Address;
    nonce: Hex;
    origin_chain_id: number;
    origin_settler: Address;
    compact: Address;
    compact_id: Hex;
    compact_expires: number;
    input_token: Address;
    input_amount: Hex;
    destination_chain_id: number;
    output_token: Address;
    minimum_output_amount: Hex;
    recipient: Address;
    destination_settler: Address;
    fill_proof_verifier: Address;
    exclusive_filler: Address;
    exclusivity_ends: number;
    fill_deadline: number;
    route_kind: number;
}

export interface SolventCompactMandate {
    order_id: Hex;
    destination_chain_id: number;
    destination_settler: Address;
    fill_proof_verifier: Address;
    output_token: Address;
    minimum_output_amount: Hex;
    recipient: Address;
    fill_deadline: number;
    exclusive_filler: Address;
    route_kind: number;
}

export interface CompactCommitmentTerms {
    arbiter: Address;
    sponsor: Address;
    nonce: Hex;
    expires: number;
    lock_tag: Hex;
    token: Address;
    amount: Hex;
    mandate: SolventCompactMandate;
}

export interface DirectOrderDraft {
    aggregate_id: Hex;
    order_id: Hex;
    compact: Address;
    order: SolventCompactOrder;
    commitment: CompactCommitmentTerms;
}

export interface CreateDirectOrderRequest {
    draft: DirectOrderDraftRequest;
    sponsor_signature: Hex;
}

export interface StepEvidence {
    command_id: Hex;
    transaction_hash?: Hex | null;
    block_number?: number | null;
    message_id?: Hex | null;
}

export type SagaState =
    | "quoted"
    | "preparing"
    | "prepared"
    | "destination_pending"
    | "destination_finalized"
    | "fill_proof_pending"
    | "origin_pending"
    | "origin_finalized"
    | "repayment_pending"
    | "complete"
    | "failed_before_delivery"
    | "needs_reconcile";

export type CrossChainLifecycleStage =
    | "quoted"
    | "destination_fill"
    | "proof_relay"
    | "origin_claim"
    | "repayment"
    | "complete";

export interface CrossChainLifecycleEvent {
    stage: CrossChainLifecycleStage;
    at: number;
}

export interface CrossChainOrder {
    order_id: Hex;
    quote: AggregateQuote;
    state: SagaState;
    lifecycle?: CrossChainLifecycleEvent[];
    origin_prepare?: Hex | null;
    destination_prepare?: Hex | null;
    destination?: StepEvidence | null;
    fill_proof?: StepEvidence | null;
    origin?: StepEvidence | null;
    repayment?: StepEvidence | null;
    failure?: string | null;
}
