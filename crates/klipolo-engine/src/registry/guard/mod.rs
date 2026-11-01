//! Source rules, static cost and diagnostics for WGSL that isn't built in, and the hardened module a device compiles
//! ([`harden`]). [`check`] decides everything that can be read from the text and naga's IR before a GPU sees the
//! shader: the text caps (`scan`, with `names`: no name naga's writer would rewrite), the registry's assembly with its
//! output check (`place`), naga's parse, the module rules (`rules`; `words`: no `@diagnostic` filter, which naga's
//! writer drops; and `host`: the host's code calls only the host's functions), naga's validation, then the static facts
//! (`analyze`, with the prelude's globals in `globals`: loop bounds in `bounds`, cost in `cost`, `weights` and `work`,
//! settings in `collect`, `flow`, `taint`, `costly`, `tally` and `settings`) and the static cost cap (`advice`). The
//! settings analysis is conservative: where tracking is uncertain, a setting is costly at both ends, so it is measured
//! at both, and so is a setting that decides more than 1,024 operations (the work tally). A colour or point setting
//! that decides work is never refused for it: it is measured at the probe set (`End::Probe`), with an Info note in
//! [`Checked::notes`]. Every refusal is a [`Diagnostic`] in the author's own lines, with a hint (`hints`) an agent can
//! act on, and with the agent's text escaped ([`shown`]).
//!
//! [`check`] runs naga, so it runs only in the render worker on text that isn't verified yet: naga's front end
//! alone can be made to allocate gigabytes (a zero-valued `array<f32, 500000000>` constant expands while parsing),
//! which only the worker's memory kill contains. The main process runs [`scan`] alone.

mod advice;
mod analyze;
mod bounds;
mod calls;
mod chains;
mod collect;
mod cost;
mod costly;
mod exits;
mod expr;
mod flow;
mod globals;
pub mod harden;
mod hints;
mod host;
mod names;
mod place;
mod rules;
mod scan;
mod settings;
mod taint;
mod tally;
mod types;
mod weights;
mod wire;
mod words;
mod work;

use std::fmt::Write as _;

use klipolo_model::contrib::ContribDef;
use klipolo_model::text_safety::invisible;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use serde::{Deserialize, Serialize};

use super::shader::ShaderKind;
#[cfg(test)]
pub(crate) use globals::TIME_MEMBERS;
pub use hints::table as hint_table;
pub(crate) use place::Placed;
pub use scan::scan;

/// The author's text, in bytes.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
pub const MAX_LINES: usize = 400;
/// Characters per line.
pub const MAX_LINE_CHARS: usize = 400;
/// `(`, `[` and `{` together, plus the `else if`s chained around a point: naga's parser is recursive. Also how deep a
/// type nests, `alias` chains included (`rule.nesting`): naga's writer and lowerer recurse on it.
pub const MAX_NESTING: usize = 32;
/// Operators and selectors (`.member`, `.xy`, `[i]`) in one statement: naga lowers and validates a chain of them
/// recursively, one stack frame per link.
pub const MAX_CHAIN: usize = 256;
pub const MAX_FUNCTIONS: usize = 64;
/// naga IR expressions over the body's functions.
pub const MAX_EXPRESSIONS: usize = 20_000;
/// Scalars in the body's constant arrays together, and in any one array or struct type in the module (a `mat4x4`
/// counts 16), nested arrays multiplied.
pub const MAX_CONST_ELEMENTS: u64 = 1024;
/// The static cost cap: a shader whose bound is over either is refused before any GPU work.
pub const MAX_OPS_PER_PIXEL: u64 = 250_000;
pub const MAX_SAMPLES_PER_PIXEL: u32 = 64;
/// Loop iterations per pixel in all, whatever the nesting: the host's shared counter.
pub const LOOP_TRIPS: u32 = 4096;
/// What a loop whose bound can't be read counts as: all of the shared counter's steps, so the static estimate is a true
/// upper bound, and the advice asks for constant bounds.
pub const UNKNOWN_LOOP_BOUND: u32 = LOOP_TRIPS;
/// Operations a millisecond in the static prediction: a fifth of the Intel Mac's peak.
pub const STATIC_OPS_PER_MS: f64 = 2e8;
/// Diagnostics from one check at most: the first few are the ones an agent fixes, and the cap keeps a body full of
/// problems from costing more than one pass.
const MAX_REPORTED: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A diagnostic's code, one of the guard's (`scan.size`, `rule.shadow`). An alias, so serde's derive doesn't take the
/// `&'static str` for a borrow from the input (`wire::code` keeps the codes it reads).
pub type Code = &'static str;

/// One problem, where the author can find it. Agent-facing, so its text is English. The app reads it from the
/// render worker's replies (`make::protocol`), where a field it doesn't have is refused and `code` has the guard's
/// shape (`wire`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    pub severity: Severity,
    /// 1-based, in the author's body; 0 when it isn't about one line (the definition, the whole text).
    pub line: u32,
    /// 1-based, in characters; 0 with line 0.
    pub column: u32,
    #[serde(deserialize_with = "wire::code")]
    pub code: Code,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The body's line, as written, with [`shown`]'s escapes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<String>,
    /// A scene's problem: where in its JSON, a JSON pointer (`/nodes/1/fill`), with [`shown`]'s escapes.
    /// Named `pointer` on the wire: no reply field is a `path`, and the app never opens it; the Make tools hand it
    /// to the agent as the problem's `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
}

/// Agent text as a diagnostic may show it: control, invisible, direction and line-break characters escaped as
/// `\u{202E}`, so a reply never reorders, hides or breaks what an agent, the app or a log shows.
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if (c.is_control() && c != '\t') || invisible(c) || matches!(c, '\u{2028}' | '\u{2029}') {
            let _ = write!(out, "\\u{{{:04X}}}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

impl Diagnostic {
    pub(crate) fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            line: 0,
            column: 0,
            code,
            message: shown(&message.into()),
            hint: None,
            source_line: None,
            pointer: None,
        }
    }

    /// A scene's problem: its JSON path, code and message, escaped.
    pub fn of_scene(p: &klipolo_model::scene::SceneProblem) -> Self {
        Self {
            pointer: Some(shown(&p.path)),
            ..Self::error(p.code, p.message.as_str())
        }
    }

    /// A note on a shader that passed.
    pub(crate) fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            ..Self::error(code, message)
        }
    }

    /// At a byte offset of the body.
    pub(crate) fn at(mut self, body: &str, offset: usize) -> Self {
        let mut offset = offset.min(body.len());
        while !body.is_char_boundary(offset) {
            offset -= 1;
        }
        let line_start = body[..offset].rfind('\n').map_or(0, |i| i + 1);
        let text = body[line_start..].split('\n').next().unwrap_or("");
        self.line = body[..offset].matches('\n').count() as u32 + 1;
        self.column = body[line_start..offset].chars().count() as u32 + 1;
        let text: String = text
            .trim_end_matches('\r')
            .chars()
            .take(MAX_LINE_CHARS)
            .collect();
        self.source_line = Some(shown(&text));
        self
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(shown(&hint.into()));
        self
    }
}

/// Which end of a setting's range costs more: `Max` is the largest number (a bool's `true`, a choice's last option).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Min,
    Max,
    /// The costly arm can't be read from a simple comparison: both ends are measured (every option of a choice).
    Both,
    /// A colour or point setting that decides work, which has no ends: it is measured at the probe set and the worst
    /// kept (points: the frame's 3×3 grid and the default; colours: the default, black, white, red, green, blue, and
    /// alpha 0 when it has alpha).
    Probe,
}

impl End {
    fn join(self, other: End) -> End {
        if self == other { self } else { End::Both }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopFact {
    /// In the author's body.
    pub line: u32,
    /// None when the bound can't be read: counted as [`UNKNOWN_LOOP_BOUND`].
    pub max_trips: Option<u32>,
    /// The setting that reaches its condition, if one does.
    pub bound_param: Option<String>,
}

/// A setting whose value reaches a loop's bound or condition, a sample count, the padding, a branch that encloses loops
/// or samples, or more than 1,024 operations' worth of branches and exits. Numbers, bools and
/// choices get their costly end; colours and points get `Probe`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostlyParam {
    pub key: String,
    pub costly_end: End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticFacts {
    /// The body's loops, by line.
    pub loops: Vec<LoopFact>,
    /// The smaller of the bound by loop bounds and the bound by the shared loop counter.
    pub ops_per_pixel: u64,
    pub samples_per_pixel: u32,
    pub ir_expressions: usize,
    pub functions: usize,
    /// Reads `kl.time`, `kl.progress`, `kl.frame` or the whole `kl`, itself or through a function it calls.
    pub animates: bool,
    /// Reaches `kl_a` or `kl_b`.
    pub reads_layer: bool,
    /// In the definition's order.
    pub costly_params: Vec<CostlyParam>,
}

impl StaticFacts {
    /// The conservative static prediction for a pass of `pixels` (the export tiling).
    pub fn static_ms(&self, pixels: u64) -> f64 {
        self.ops_per_pixel as f64 * pixels as f64 / STATIC_OPS_PER_MS
    }
}

/// A shader that passed: its assembled source (with the output check, `shader::SANITIZE`), where the body starts in
/// it, naga's module and info, and its facts. [`harden::harden`] makes the text a device compiles from it.
#[derive(Debug)]
pub struct Checked {
    /// The assembled text `check` read, before hardening. Nothing compiles it: it has no loop counter, and the
    /// hardened module (`ShaderEffect::source`) is the only form any device compiles.
    pub source: String,
    /// The first line of the body in `source`, 1-based.
    pub body_line: u32,
    pub module: naga::Module,
    pub info: naga::valid::ModuleInfo,
    pub facts: StaticFacts,
    /// Info diagnostics on a shader that passed: each colour or point setting measured at probes, where it decides work
    /// and how to avoid it.
    pub notes: Vec<Diagnostic>,
}

/// Checks a body of the kind `kind` for the definition `def`, as every shader that isn't built in is checked.
pub fn check(def: &ContribDef, kind: ShaderKind, body: &str) -> Result<Checked, Vec<Diagnostic>> {
    let scanned = scan::scan(body);
    if !scanned.is_empty() {
        return Err(scanned);
    }
    let (source, host, start) = place::assemble(def, kind, body)?;
    let p = Placed { body, start, host };
    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| vec![place::parse_error(&p, kind, &e)])?;
    let mut refused = rules::check(&p, kind, &module);
    if refused.is_empty() {
        refused = words::check(&p);
    }
    if refused.is_empty() {
        refused = host::check(&p, kind, &module);
    }
    if !refused.is_empty() {
        return Err(refused);
    }
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| vec![place::validation_error(&p, kind, &e)])?;
    let (facts, refused, notes) = analyze::facts(&p, def, kind, &module, &info);
    if !refused.is_empty() {
        return Err(refused);
    }
    let body_line = source[..start].matches('\n').count() as u32 + 1;
    Ok(Checked {
        source,
        body_line,
        module,
        info,
        facts,
        notes,
    })
}

/// Whether a built-in animates, read from its IR as [`check`]'s facts read it (at registry build; built-ins skip the
/// caps and rules). One that doesn't parse or validate counts as animating.
pub(crate) fn animates(def: &ContribDef, kind: ShaderKind, source: &str, body: &str) -> bool {
    let Some((host, start)) = place::body_start(def, source, body) else {
        return true;
    };
    let Ok(module) = naga::front::wgsl::parse_str(source) else {
        return true;
    };
    let Ok(info) = Validator::new(ValidationFlags::all(), Capabilities::empty()).validate(&module)
    else {
        return true;
    };
    analyze::facts(&Placed { body, start, host }, def, kind, &module, &info)
        .0
        .animates
}

/// Calls `visit` with every statement of `block`, nested ones included (loop bodies and `continuing`, blocks, both
/// arms of an `if`, every `switch` case).
fn each_statement<'a>(block: &'a naga::Block, visit: &mut impl FnMut(&'a naga::Statement)) {
    use naga::Statement as S;
    for s in block.iter() {
        visit(s);
        match s {
            S::Loop {
                body, continuing, ..
            } => {
                each_statement(body, visit);
                each_statement(continuing, visit);
            }
            S::Block(b) => each_statement(b, visit),
            S::If { accept, reject, .. } => {
                each_statement(accept, visit);
                each_statement(reject, visit);
            }
            S::Switch { cases, .. } => cases.iter().for_each(|c| each_statement(&c.body, visit)),
            _ => {}
        }
    }
}
