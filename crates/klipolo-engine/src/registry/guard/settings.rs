//! The facts' costly settings from the entry function's summary. A setting is
//! costly at the end its conditions read (`Summary::costly`), at both ends when it reaches a pointer handed to a helper
//! (`via_pointer`) or decides more than `HEAVY_ARM` operations (`decides`, the work tally), and at `Max` as the
//! padding's setting. Reported in the definition's order.
//!
//! A colour or point setting has no ends, so one that is costly in any of these ways is marked `Probe`: it is
//! measured at the probe set. It is never refused for it. Each gets an Info
//! note per reason, at the line where it decides the most work (`Summary::at`), with a hint that fits its kind.

use std::collections::HashMap;

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::ParamKind;
use naga::Module;

use super::analyze::Summary;
use super::globals::{Class, Globals, class};
use super::taint::Source;
use super::tally::HEAVY_ARM;
use super::{CostlyParam, Diagnostic, End, MAX_REPORTED, Placed};

/// A colour or point setting that decides how much work a pixel does.
struct Probed<'d> {
    key: &'d str,
    kind: &'d ParamKind,
    member: u32,
    /// Through a condition or the tally, not only through a pointer.
    decides: bool,
}

pub(super) fn costly(
    p: &Placed,
    def: &ContribDef,
    g: &Globals,
    m: &Module,
    e: &Summary,
) -> (Vec<CostlyParam>, Vec<Diagnostic>) {
    let decided = e.decides.iter().filter(|&&(_, w)| w > HEAVY_ARM);
    let reasons = e
        .costly
        .iter()
        .map(|&(s, end)| (s, end, true))
        .chain(decided.map(|&(s, _)| (s, End::Both, true)))
        .chain(e.via_pointer.iter().map(|&(s, _)| (s, End::Both, false)));
    let mut ends: HashMap<&str, End> = HashMap::new();
    let mut probed: Vec<Probed> = Vec::new();
    for (source, end, decides) in reasons {
        let Source::Param(k) = source else { continue };
        let Some(spec) = g.keys.get(k as usize).and_then(|(key, _)| def.param(key)) else {
            continue;
        };
        if class(&spec.kind) == Class::Other {
            match probed.iter_mut().find(|r| r.key == spec.key) {
                Some(r) => r.decides |= decides,
                None => probed.push(Probed {
                    key: &spec.key,
                    kind: &spec.kind,
                    member: k,
                    decides,
                }),
            }
            ends.insert(spec.key.as_str(), End::Probe);
            continue;
        }
        ends.entry(spec.key.as_str())
            .and_modify(|x| *x = x.join(end))
            .or_insert(end);
    }
    if let Some(key) = def.padding.as_ref().and_then(|pad| pad.param.as_deref())
        && let Some(spec) = def.param(key)
        && class(&spec.kind) != Class::Other
    {
        ends.entry(spec.key.as_str())
            .and_modify(|x| *x = x.join(End::Max))
            .or_insert(End::Max);
    }
    let costly = def
        .params
        .iter()
        .filter_map(|s| {
            ends.get(s.key.as_str()).map(|&costly_end| CostlyParam {
                key: s.key.clone(),
                costly_end,
            })
        })
        .collect();
    (costly, notes(p, m, e, &probed))
}

/// The notes on the settings measured at probes, at most `MAX_REPORTED`.
fn notes(p: &Placed, m: &Module, e: &Summary, probed: &[Probed]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for r in probed {
        if out.len() >= MAX_REPORTED {
            break;
        }
        let (key, source) = (r.key, Source::Param(r.member));
        let (what, set) = match r.kind {
            ParamKind::Point { .. } => (
                "point",
                "probe points: the frame's 3×3 grid and its default",
            ),
            ParamKind::Color { alpha: true, .. } => (
                "colour",
                "probe colours: its default, black, white, red, green, blue and clear",
            ),
            ParamKind::Color { .. } => (
                "colour",
                "probe colours: its default, black, white, red, green and blue",
            ),
            _ => ("", "probe values"),
        };
        let named = if what.is_empty() {
            format!("`{key}`")
        } else {
            format!("{what} setting `{key}`")
        };
        if r.decides {
            let at =
                e.at.iter()
                    .find(|(s, _)| *s == source)
                    .map(|(_, w)| w.offset);
            let d = Diagnostic::info(
                "cost.probe",
                format!(
                    "The {named} decides how much work a pixel does{}, so it is measured at the {set}.",
                    if at.is_some() { " here" } else { "" }
                ),
            );
            let d = if e.whole_params {
                // Review m-d: a whole copy of `params` makes every setting it reaches count.
                d.hint(format!("Read `params.{key}` (and each setting you need) where you use it, instead of copying `params` or passing it to a function: then only the settings that decide work are probed."))
            } else {
                d.hint(format!("Nothing to change. To keep it out of the probes, choose with `select()`, `mix()` or `step()` instead of an `if` (or an early `return`) on a value this {} reaches.", if what.is_empty() { "setting" } else { what }))
            };
            out.push(match at {
                Some(at) => d.at(p.body, at),
                None => d,
            });
        }
        let helper = e
            .via_pointer
            .iter()
            .find(|(s, _)| *s == source)
            .and_then(|(_, h)| m.functions[*h].name.as_deref());
        if let Some(name) = helper
            && out.len() < MAX_REPORTED
        {
            // A pointer passed to a helper counts as deciding work, whatever the helper does with it.
            out.push(
                Diagnostic::info(
                    "cost.probe",
                    format!(
                        "The {named} reaches a pointer passed to `{name}`, which counts as deciding how much work a \
                         pixel does, so it is measured at the {set}."
                    ),
                )
                .hint(format!(
                    "Pass the value to `{name}` by value and use what it returns, instead of a pointer, to keep it \
                     out of the probes."
                )),
            );
        }
    }
    out
}
