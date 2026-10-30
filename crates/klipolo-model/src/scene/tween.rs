//! The timeline's tweens: GSAP-shaped data (never GSAP's code), compiled with the nodes' own keys into one set of
//! keys when the scene loads.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Val;

/// One GSAP-shaped tween: exactly one of `to`, `from`, `fromTo` (with `start`) or `set`, the nodes it animates, when
/// (`at`), how long (`dur`) and how (`ease`), and the property values (everything else, `vars`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tween {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Selectors>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Selectors>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_to: Option<Selectors>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Selectors>,
    /// `fromTo`'s start values; `vars` are its end values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<BTreeMap<String, Val>>,
    /// Seconds, `"<"` (the previous tween's start), `">"` (its end), `"+=s"` and `"-=s"` (from the timeline's end),
    /// a label defined earlier, `"out"` (the exit segment) or `"out+s"`; the timeline's end when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Val>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<StaggerArg>,
    /// Extra runs; −1 repeats until the scene ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yoyo: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_delay: Option<f64>,
    /// Names this tween's start for later `at`s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The properties it animates and their values: `{"y": "22px", "opacity": 0}`.
    #[serde(flatten)]
    pub vars: BTreeMap<String, Val>,
}

/// `"#id"`, `".class"` or a node type (`"text"`), or a list of them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Selectors {
    One(String),
    Many(Vec<String>),
}

/// Seconds between targets (or split units), or the whole spread: `0.05` or
/// `{"each": 0.05, "from": "center"}` or `{"amount": 0.6, "from": "random", "seed": 7}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StaggerArg {
    Each(f64),
    Spec {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        each: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<StaggerFrom>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StaggerFrom {
    Start,
    End,
    Center,
    Edges,
    Random,
}
