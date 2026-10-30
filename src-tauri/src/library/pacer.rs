//! Request spacing for the web services the library talks to.

use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Holds each caller until at least `spacing` has passed since the
/// previous one was let through. Concurrent callers queue on the mutex,
/// so they are spaced too.
pub struct Pacer {
    spacing: Duration,
    last: Mutex<Option<Instant>>,
}

impl Pacer {
    pub fn new(spacing: Duration) -> Self {
        Self {
            spacing,
            last: Mutex::new(None),
        }
    }

    pub async fn wait(&self) {
        let mut last = self.last.lock().await;
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.spacing {
                tokio::time::sleep(self.spacing - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn spaces_consecutive_callers() {
        let pacer = Pacer::new(Duration::from_secs(1));
        let start = Instant::now();
        pacer.wait().await;
        pacer.wait().await;
        pacer.wait().await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }
}
