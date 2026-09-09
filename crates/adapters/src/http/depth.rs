//! Bounded execution for CPU-heavy depth reads.

use std::sync::Arc;

use axum::http::StatusCode;
use solvent_core::pool::{DepthService, PoolDepth};
use tokio::sync::Semaphore;

use super::primitives::Response;

/// Bounds retained HTTP work as well as the smaller set of active CPU workers.
const MAX_OUTSTANDING_REQUESTS: usize = 32;

/// Keeps depth computation off the async executor and applies backpressure before work queues grow.
#[derive(Clone)]
pub struct DepthReader {
    service: Arc<DepthService>,
    admission: Arc<Semaphore>,
    workers: Arc<Semaphore>,
}

impl DepthReader {
    pub fn new(service: Arc<DepthService>) -> Self {
        let worker_count = std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(1).clamp(1, 4));
        Self::with_limits(service, MAX_OUTSTANDING_REQUESTS, worker_count)
    }

    fn with_limits(service: Arc<DepthService>, request_limit: usize, worker_count: usize) -> Self {
        Self {
            service,
            admission: Arc::new(Semaphore::new(request_limit)),
            workers: Arc::new(Semaphore::new(worker_count)),
        }
    }

    pub(super) async fn read(
        &self,
        read: impl FnOnce(&DepthService) -> Option<PoolDepth> + Send + 'static,
    ) -> Result<Option<PoolDepth>, Response<()>> {
        let admission = self.admission.clone().try_acquire_owned().map_err(|_| {
            Response::error(
                "depth request capacity reached",
                StatusCode::SERVICE_UNAVAILABLE,
            )
        })?;
        let worker = self
            .workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_error| {
                solvent_core::obs::error!(error = %_error, "depth worker pool closed");
                Response::error("depth worker unavailable", StatusCode::SERVICE_UNAVAILABLE)
            })?;
        let service = Arc::clone(&self.service);
        tokio::task::spawn_blocking(move || {
            // A cancelled request must retain both permits until its CPU job actually stops.
            let _admission = admission;
            let _worker = worker;
            read(&service)
        })
        .await
        .map_err(|_error| {
            solvent_core::obs::error!(error = %_error, "depth computation failed");
            Response::error(
                "depth computation failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn reader(request_limit: usize) -> DepthReader {
        let service = super::super::tests::test_state().depth.service;
        DepthReader::with_limits(service, request_limit, 1)
    }

    #[tokio::test]
    async fn running_job_retains_worker_slot_after_request_cancellation() {
        let reader = reader(MAX_OUTSTANDING_REQUESTS);
        let workers = Arc::clone(&reader.workers);
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            reader
                .read(move |_| {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(2)).unwrap();
                    finished.send(()).unwrap();
                    None
                })
                .await
        });
        running.await.unwrap();

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(workers.available_permits(), 0);

        release.send(()).unwrap();
        done.await.unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(2), workers.acquire_owned())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    #[tokio::test]
    async fn admission_bounds_waiters_and_recovers_after_cancellation() {
        let reader = reader(MAX_OUTSTANDING_REQUESTS);
        let running_reader = reader.clone();
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            running_reader
                .read(move |_| {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(2)).unwrap();
                    finished.send(()).unwrap();
                    None
                })
                .await
        });
        running.await.unwrap();
        let mut queued: Vec<_> = (1..MAX_OUTSTANDING_REQUESTS)
            .map(|_| Box::pin(reader.read(|_| None)))
            .collect();
        for request in &mut queued {
            assert!(futures::poll!(request.as_mut()).is_pending());
        }

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let mut rejected = Box::pin(reader.read(|_| panic!("rejected work must never start")));
        let std::task::Poll::Ready(Err(error)) = futures::poll!(rejected.as_mut()) else {
            panic!("a saturated reader must reject immediately, including cancelled CPU work");
        };
        assert_eq!(error.status_code, StatusCode::SERVICE_UNAVAILABLE);

        drop(queued.pop());
        let mut replacement = Box::pin(reader.read(|_| None));
        assert!(futures::poll!(replacement.as_mut()).is_pending());
        drop(queued);
        drop(replacement);
        release.send(()).unwrap();
        done.await.unwrap();
        assert!(reader.read(|_| None).await.unwrap().is_none());
    }
}
