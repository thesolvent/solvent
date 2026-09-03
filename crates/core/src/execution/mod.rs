//! The execution slice: turn a reserved plan into an included fill — simulate, submit privately,
//! track, and couple the outcome back to the ledger (post the actual pulled amounts on confirm,
//! void on failure).

pub mod service;

pub use service::ExecutionService;
