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

export interface StepEvidence {
    command_id: Hex;
    transaction_hash?: Hex;
    block_number?: number;
    message_id?: Hex;
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

export interface CrossChainOrder {
    order_id: Hex;
    quote: AggregateQuote;
    state: SagaState;
    origin_prepare?: Hex;
    destination_prepare?: Hex;
    destination?: StepEvidence;
    fill_proof?: StepEvidence;
    origin?: StepEvidence;
    repayment?: StepEvidence;
    failure?: string;
}
