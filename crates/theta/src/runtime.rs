//! The bridge between GPUI's single-threaded UI executor and Tokio.
//!
//! Database drivers are Tokio-based (sqlx-style clients, SSH tunnels), and GPUI
//! renders on the main thread, so I/O must never be awaited on the UI thread.
//! The pattern used everywhere in the app:
//!
//! ```ignore
//! let work = runtime::spawn(async move { connection.execute(&sql, &opts).await });
//! cx.spawn(async move |this, cx| {
//!     let result = work.await;                    // awaited on GPUI's executor
//!     this.update(cx, |this, cx| { /* apply */ }).ok();
//! }).detach();
//! ```
//!
//! [`spawn`] runs the future on a small global multi-thread runtime and returns
//! its `JoinHandle`, which is a plain `Future` that GPUI's executor can poll
//! (Tokio wakes the registered waker from its worker thread, and GPUI's waker
//! schedules the continuation back on the main thread). Dropping the returned
//! future does *not* abort the task; call [`Spawned::abort`] to cancel.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use theta_core::Error;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The shared runtime: two workers are plenty, database work is I/O-bound.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("theta-io")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// A task running on the Tokio runtime. Awaiting it yields the task's output;
/// a panic inside the task surfaces as `Error::Other` instead of unwinding the UI.
pub struct Spawned<T> {
    handle: tokio::task::JoinHandle<T>,
}

impl<T> Spawned<T> {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

impl<T> Future for Spawned<T> {
    type Output = Result<T, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.handle).poll(cx) {
            Poll::Ready(Ok(v)) => Poll::Ready(Ok(v)),
            Poll::Ready(Err(e)) if e.is_cancelled() => Poll::Ready(Err(Error::Cancelled)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(Error::Other(format!("background task failed: {e}")))),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub fn spawn<F, T>(future: F) -> Spawned<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    Spawned { handle: runtime().spawn(future) }
}

/// Like [`spawn`] for futures that themselves return `Result<_, Error>`:
/// flattens the join error into the same error type.
pub async fn spawn_result<F, T>(future: F) -> Result<T, Error>
where
    F: Future<Output = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
    spawn(future).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawned_future_resolves_on_another_executor() {
        // Poll the JoinHandle from a plain thread executor, as GPUI would.
        let value = pollster_block_on(async { spawn(async { 21 * 2 }).await });
        assert_eq!(value.unwrap(), 42);
    }

    #[test]
    fn panics_become_errors() {
        let r = pollster_block_on(async { spawn(async { panic!("boom") }).await });
        let r: Result<(), Error> = r;
        assert!(matches!(r, Err(Error::Other(m)) if m.contains("background task failed")));
    }

    #[test]
    fn spawn_result_flattens() {
        let r = pollster_block_on(spawn_result(async { Err::<(), _>(Error::Closed) }));
        assert!(matches!(r, Err(Error::Closed)));
    }

    /// Minimal single-future executor so tests do not need another dependency.
    fn pollster_block_on<F: Future>(fut: F) -> F::Output {
        use std::sync::Arc;
        use std::task::{Wake, Waker};
        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park();
        }
    }
}
