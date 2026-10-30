//! The app's one engine: started on the first attach and kept for the app's lifetime, but started again
//! when the old one has ended (its actor panicked or was shut down), so Retry always reaches a live engine.

use std::future::Future;
use std::sync::{Mutex, PoisonError};

use super::Engine;

#[derive(Default)]
pub struct EngineSlot {
    engine: Mutex<Option<Engine>>,
    /// Held while an engine starts, so two attaches never start two.
    starting: tokio::sync::Mutex<()>,
}

impl EngineSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// The live engine, if one is running (commands other than attach never start one).
    pub fn running(&self) -> Option<Engine> {
        self.lock().clone().filter(Engine::is_running)
    }

    /// The live engine, or a new one from `start` when there is none or the old one has ended. A failed start is
    /// tried again on the next call.
    pub async fn get_or_start<F, Fut>(&self, start: F) -> Result<Engine, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Engine, String>>,
    {
        let _starting = self.starting.lock().await;
        if let Some(engine) = self.running() {
            return Ok(engine);
        }
        let engine = start().await?;
        *self.lock() = Some(engine.clone());
        Ok(engine)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Engine>> {
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
