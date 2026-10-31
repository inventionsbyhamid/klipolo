//! What chat has cost this session: its messages (`PlanOut::usage_usd`, and what a failed message's calls cost) and
//! the pictures drawn for it. Nothing is shown per message; Settings shows the session's AI spend.

use std::sync::{Mutex, PoisonError};

use crate::credentials::SessionSpend;

/// Chat's spend this session, in US dollars (Settings shows it in `SessionSpend`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChatSpend {
    /// What the messages' model calls cost.
    pub chat_usd: f64,
    /// The messages that reached the model.
    pub messages: u32,
    /// What the pictures drawn for chat cost.
    pub pictures_usd: f64,
    /// The pictures asked for.
    pub pictures: u32,
}

impl ChatSpend {
    pub fn total_usd(&self) -> f64 {
        self.chat_usd + self.pictures_usd
    }

    /// The session's AI spend Settings shows: this, with what the analysis cost.
    pub fn with_analysis(&self, analysis_usd: f64) -> SessionSpend {
        SessionSpend {
            analysis_usd,
            chat_usd: self.chat_usd,
            chat_messages: self.messages,
            pictures_usd: self.pictures_usd,
            pictures: self.pictures,
        }
    }
}

/// The session's chat spend, counted as it happens.
#[derive(Default)]
pub struct SpendCounter(Mutex<ChatSpend>);

impl SpendCounter {
    pub fn now(&self) -> ChatSpend {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A message that reached the model (`model_calls` > 0) and cost `usd`.
    pub(crate) fn message(&self, usd: f64, model_calls: usize) {
        let mut spend = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if model_calls > 0 {
            spend.messages += 1;
        }
        spend.chat_usd += usd.max(0.0);
    }
}
