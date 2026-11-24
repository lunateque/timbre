use std::io;
use tokio::sync::{RwLock, watch};

pub(super) struct Requests {
    cancelled: watch::Sender<bool>,
    active: RwLock<()>,
}

impl Requests {
    pub(super) fn new() -> Self {
        Self {
            cancelled: watch::channel(false).0,
            active: RwLock::new(()),
        }
    }

    pub(super) async fn run<T>(&self, request: impl Future<Output = T>) -> io::Result<T> {
        let _active = self.active.read().await;
        let work = request;
        let mut cancelled = self.cancelled.subscribe();
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|stop| *stop) => Err(io::ErrorKind::Interrupted.into()),
            result = work => Ok(result),
        }
    }

    pub(super) async fn finish(&self) {
        // Hyper can run HTTP/2 service futures separately from their connection.
        self.cancelled.send_replace(true);
        drop(self.active.write().await);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::oneshot;

    struct OnDrop(Arc<AtomicBool>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn finish_waits_for_a_detached_request_to_drop_and_refuses_late_requests() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let requests = Arc::new(Requests::new());
                let done = Arc::new(AtomicBool::new(false));
                let on_drop = OnDrop(Arc::clone(&done));
                let (started, wait) = oneshot::channel();
                let worker = Arc::clone(&requests);
                let active = tokio::spawn(async move {
                    worker
                        .run(async move {
                            let _on_drop = on_drop;
                            started.send(()).unwrap();
                            pending::<()>().await;
                        })
                        .await
                });
                wait.await.unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(5), requests.finish())
                    .await
                    .unwrap();
                assert!(done.load(Ordering::Acquire));
                assert_eq!(
                    active.await.unwrap().unwrap_err().kind(),
                    io::ErrorKind::Interrupted
                );
                let late = requests.run(async { panic!("late request ran") }).await;
                assert_eq!(late.unwrap_err().kind(), io::ErrorKind::Interrupted);
            });
    }
}
