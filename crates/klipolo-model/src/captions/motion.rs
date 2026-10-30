//! How a caption style's words and pages come in and go: part of the style vocabulary
//! (`style::*` re-exports it).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::style_check::{ms, range, Checked};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionEntrance")]
pub struct Entrance {
    pub per: Per,
    pub motion: Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionPer")]
pub enum Per {
    Word,
    Page,
}

/// How a word or page comes in; the easings are fixed (`crate::motion::Ease`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[ts(export, rename = "CaptionMove")]
pub enum Move {
    None,
    Fade {
        ms: u32,
    },
    Rise {
        ms: u32,
        dy_em: f64,
    },
    Pop {
        ms: u32,
        from: f64,
        peak: f64,
        dy_em: f64,
    },
    Slam {
        ms: u32,
        from: f64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionShakeDef")]
pub struct ShakeDef {
    pub amp_em: f64,
    pub ms: u32,
    pub keyword_pages_only: bool,
}

/// How a page goes: a cut, or a fade when the gap to the next page is at least `min_gap_ms`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[ts(export, rename = "CaptionExit")]
pub enum Exit {
    Cut,
    Fade { ms: u32, min_gap_ms: u32 },
}

// Their rules, as `CaptionStyleDef::check` names them.
impl Entrance {
    pub(super) fn check(&self) -> Checked {
        match self.motion {
            Move::None => Ok(()),
            Move::Fade { ms: m } => ms(m, 1_000, "enter.motion.ms"),
            Move::Rise { ms: m, dy_em } => {
                ms(m, 1_000, "enter.motion.ms")?;
                range(dy_em, -1.0, 1.0, "enter.motion.dyEm")
            }
            Move::Pop {
                ms: m,
                from,
                peak,
                dy_em,
            } => {
                ms(m, 1_000, "enter.motion.ms")?;
                range(from, 0.0, 2.0, "enter.motion.from")?;
                range(peak, 0.5, 2.0, "enter.motion.peak")?;
                range(dy_em, -1.0, 1.0, "enter.motion.dyEm")
            }
            Move::Slam { ms: m, from } => {
                ms(m, 1_000, "enter.motion.ms")?;
                range(from, 1.0, 3.0, "enter.motion.from")
            }
        }
    }
}

impl ShakeDef {
    pub(super) fn check(&self) -> Checked {
        range(self.amp_em, 0.0, 0.1, "shake.ampEm")?;
        ms(self.ms, 500, "shake.ms")
    }
}

impl Exit {
    pub(super) fn check(&self) -> Checked {
        match *self {
            Exit::Cut => Ok(()),
            Exit::Fade { ms: m, min_gap_ms } => {
                ms(m, 1_000, "exit.ms")?;
                ms(min_gap_ms, 10_000, "exit.minGapMs")
            }
        }
    }
}
