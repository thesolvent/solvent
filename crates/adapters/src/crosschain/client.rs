use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use solvent_core::deps::crosschain::{RemoteProgress, RemoteSolvent, RemoteSolventError};
use solvent_core::primitives::crosschain::{
    ChainExecutionPlan, DirectExecutionPlans, DirectOrderAuthorization, LegQuote, LegQuoteRequest,
    Preparation, RemoteCommand, StepValidationContext,
};
use solvent_core::primitives::{
    AggregateQuoteId, CrossChainOrderId, CrossChainStepId, PrepareToken,
};
use std::time::Duration;

/// HTTP adapter for a private chain-local Solvent API. It never logs response bodies.
pub struct SolventClient {
    client: Client,
    base_url: Url,
}

impl SolventClient {
    pub fn from_base_url(base_url: &str, bearer_token: &str) -> Result<Self, RemoteSolventError> {
        let base_url = Url::parse(base_url)
            .map_err(|error| RemoteSolventError::InvalidResponse(error.to_string()))?;
        let mut value = HeaderValue::from_str(&format!("Bearer {bearer_token}"))
            .map_err(|error| RemoteSolventError::InvalidResponse(error.to_string()))?;
        value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, value);
        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| RemoteSolventError::InvalidResponse(error.to_string()))?;
        Ok(Self { client, base_url })
    }

    async fn post<Request, Response>(
        &self,
        path: &str,
        request: &Request,
    ) -> Result<Response, RemoteSolventError>
    where
        Request: Serialize + Sync,
        Response: serde::de::DeserializeOwned,
    {
        let url = self
            .base_url
            .join(path)
            .map_err(|error| RemoteSolventError::InvalidResponse(error.to_string()))?;
        let response = self
            .client
            .post(url)
            .json(request)
            .send()
            .await
            .map_err(|error| RemoteSolventError::Unavailable(error.to_string()))?;
        if response.status().is_success() {
            return response
                .json()
                .await
                .map_err(|error| RemoteSolventError::InvalidResponse(error.to_string()));
        }
        let status = response.status();
        let message = status_message(status, response.text().await.ok());
        if status.is_client_error() {
            Err(RemoteSolventError::Rejected(message))
        } else {
            Err(RemoteSolventError::Unavailable(message))
        }
    }
}

#[derive(Serialize)]
struct PrepareRequest<'a> {
    aggregate_id: AggregateQuoteId,
    quote: &'a LegQuote,
}

#[derive(Serialize)]
struct StageRequest<'a> {
    context: &'a StepValidationContext,
    plan: &'a ChainExecutionPlan,
}

#[derive(Serialize)]
struct CompletionStageRequest<'a> {
    context: &'a StepValidationContext,
    plan: &'a ChainExecutionPlan,
    preparation: PrepareToken,
}

#[derive(Serialize)]
struct TokenRequest {
    token: PrepareToken,
}

#[derive(Serialize)]
struct CommandRequest {
    order_id: CrossChainOrderId,
    command_id: CrossChainStepId,
    command: RemoteCommand,
    preparation: PrepareToken,
}

#[async_trait]
impl RemoteSolvent for SolventClient {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, RemoteSolventError> {
        self.post("internal/v1/cross-chain/leg-quotes", request)
            .await
    }

    async fn author_direct(
        &self,
        authorization: &DirectOrderAuthorization,
    ) -> Result<DirectExecutionPlans, RemoteSolventError> {
        self.post("internal/v1/cross-chain/direct-plans", authorization)
            .await
    }

    async fn stage(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/stage",
            &StageRequest { context, plan },
        )
        .await
    }

    async fn stage_cctp_completion(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
        preparation: PrepareToken,
    ) -> Result<(), RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/stage-cctp-completion",
            &CompletionStageRequest {
                context,
                plan,
                preparation,
            },
        )
        .await
    }

    async fn prepare(
        &self,
        aggregate_id: AggregateQuoteId,
        quote: &LegQuote,
    ) -> Result<Preparation, RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/preparations",
            &PrepareRequest {
                aggregate_id,
                quote,
            },
        )
        .await
    }

    async fn commit(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/preparations/commit",
            &TokenRequest { token },
        )
        .await
    }

    async fn inspect(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/preparations/inspect",
            &TokenRequest { token },
        )
        .await
    }

    async fn release(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/preparations/release",
            &TokenRequest { token },
        )
        .await
    }

    async fn command(
        &self,
        order_id: CrossChainOrderId,
        command_id: CrossChainStepId,
        command: RemoteCommand,
        preparation: PrepareToken,
    ) -> Result<RemoteProgress, RemoteSolventError> {
        self.post(
            "internal/v1/cross-chain/steps",
            &CommandRequest {
                order_id,
                command_id,
                command,
                preparation,
            },
        )
        .await
    }
}

/// The refusal as the chain-local service worded it.
///
/// Its own sentence is what a caller can act on; the status code only names the layer that said
/// no. The code is kept for the case where a service refuses without saying why.
fn status_message(status: StatusCode, body: Option<String>) -> String {
    match body
        .map(|body| body.trim().to_string())
        .filter(|b| !b.is_empty())
    {
        Some(body) => body,
        None => format!("chain-local Solvent returned HTTP {status}"),
    }
}
