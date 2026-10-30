//! Ease names to `Ease` values and back, for scenes and the tools. Klipolo reads GSAP's
//! and CSS's names for the eases it has; the curves are its own, from the published equations, never GSAP's code. A
//! parametric GSAP form such as `back.out(2)` reads as its default, with a note for the agent. Names are matched
//! without regard to ASCII case, so GSAP 2's `Power2.easeOut` reads too.

use crate::motion::spring::{self, DAMPING, MASS, MIN_SPRING_ZETA, STIFFNESS};
use crate::motion::{Ease as E, EASE_Y, MAX_STEPS};

/// The longest name Klipolo reads; every name [`E::name`] writes is shorter.
pub const MAX_NAME_LEN: usize = 128;

/// Names that stand alone: GSAP's `none`, CSS's keywords and Klipolo's `hold`.
const WORDS: [(&str, E); 7] = [
    ("none", E::Linear),
    ("linear", E::Linear),
    ("ease", E::Ease),
    ("ease-in", E::EaseIn),
    ("ease-out", E::EaseOut),
    ("ease-in-out", E::EaseInOut),
    ("hold", E::Hold),
];

/// The three kinds after a GSAP family's dot, in the order [`FAMILIES`] lists their eases.
const KINDS: [&str; 3] = ["in", "out", "inOut"];

const POWER1: [E; 3] = [E::Power1In, E::Power1Out, E::Power1InOut];
const POWER2: [E; 3] = [E::Power2In, E::Power2Out, E::Power2InOut];
const POWER3: [E; 3] = [E::Power3In, E::Power3Out, E::Power3InOut];
const POWER4: [E; 3] = [E::Power4In, E::Power4Out, E::Power4InOut];

/// GSAP's families with their `.in`, `.out` and `.inOut` eases; a bare family name is its `.out`, as in GSAP.
/// `linear` is GSAP's name for `power0`, and `quad`, `cubic`, `quart`, `quint` and `strong` its aliases of
/// `power1`–`power4`. [`E::name`] writes GSAP's own `power` and family names.
const FAMILIES: [(&str, [E; 3]); 17] = [
    ("power0", [E::Linear; 3]),
    ("linear", [E::Linear; 3]),
    ("power1", POWER1),
    ("power2", POWER2),
    ("power3", POWER3),
    ("power4", POWER4),
    ("quad", POWER1),
    ("cubic", POWER2),
    ("quart", POWER3),
    ("quint", POWER4),
    ("strong", POWER4),
    ("sine", [E::SineIn, E::SineOut, E::SineInOut]),
    ("expo", [E::ExpoIn, E::ExpoOut, E::ExpoInOut]),
    ("circ", [E::CircIn, E::CircOut, E::CircInOut]),
    ("back", [E::BackIn, E::BackOut, E::BackInOut]),
    ("elastic", [E::ElasticIn, E::ElasticOut, E::ElasticInOut]),
    ("bounce", [E::BounceIn, E::BounceOut, E::BounceInOut]),
];

impl E {
    /// The ease `name` names, with an English note for agents when Klipolo read a parametric GSAP form as its default;
    /// None for a name Klipolo doesn't read or an ease past its bounds ([`read`] says why).
    pub fn from_name(name: &str) -> Option<(E, Option<String>)> {
        read(name).ok()
    }

    /// The ease's name, GSAP's where it has one (`none`, `power3.inOut`, `back.out`), else CSS's (`ease-in`,
    /// `steps(4)`, `cubic-bezier(0.2, 0, 0, 1)`), else Klipolo's (`spring(120, 14, 1)`, `hold`). A valid ease's name
    /// reads back as the same ease through [`E::from_name`]. Every variant is named here, so a new one can't be
    /// written as another's name.
    pub fn name(&self) -> String {
        let fixed = match *self {
            E::Steps { n } => return format!("steps({n})"),
            E::CubicBezier { x1, y1, x2, y2 } => {
                return format!(
                    "cubic-bezier({}, {}, {}, {})",
                    exact(x1),
                    exact(y1),
                    exact(x2),
                    exact(y2)
                )
            }
            E::Spring {
                stiffness,
                damping,
                mass,
            } => {
                return format!(
                    "spring({}, {}, {})",
                    exact(stiffness),
                    exact(damping),
                    exact(mass)
                )
            }
            E::Linear => "none",
            E::Ease => "ease",
            E::EaseIn => "ease-in",
            E::EaseOut => "ease-out",
            E::EaseInOut => "ease-in-out",
            E::Hold => "hold",
            E::Power1In => "power1.in",
            E::Power1Out => "power1.out",
            E::Power1InOut => "power1.inOut",
            E::Power2In => "power2.in",
            E::Power2Out => "power2.out",
            E::Power2InOut => "power2.inOut",
            E::Power3In => "power3.in",
            E::Power3Out => "power3.out",
            E::Power3InOut => "power3.inOut",
            E::Power4In => "power4.in",
            E::Power4Out => "power4.out",
            E::Power4InOut => "power4.inOut",
            E::SineIn => "sine.in",
            E::SineOut => "sine.out",
            E::SineInOut => "sine.inOut",
            E::ExpoIn => "expo.in",
            E::ExpoOut => "expo.out",
            E::ExpoInOut => "expo.inOut",
            E::CircIn => "circ.in",
            E::CircOut => "circ.out",
            E::CircInOut => "circ.inOut",
            E::BackIn => "back.in",
            E::BackOut => "back.out",
            E::BackInOut => "back.inOut",
            E::ElasticIn => "elastic.in",
            E::ElasticOut => "elastic.out",
            E::ElasticInOut => "elastic.inOut",
            E::BounceIn => "bounce.in",
            E::BounceOut => "bounce.out",
            E::BounceInOut => "bounce.inOut",
        };
        fixed.to_owned()
    }
}

/// What `name` means (the ease, and a note when a parametric GSAP form was read as its default), or why Klipolo can't
/// read it, in English for the agent.
pub fn read(name: &str) -> Result<(E, Option<String>), String> {
    let name = name.trim();
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "An ease name is at most {MAX_NAME_LEN} characters."
        ));
    }
    let Some((head, rest)) = name.split_once('(') else {
        return fixed(name).map(|e| (e, None)).ok_or_else(|| unknown(name));
    };
    let Some(args) = rest.strip_suffix(')') else {
        return Err(unknown(name));
    };
    let head = head.trim();
    let args: Vec<&str> = match args.trim() {
        "" => Vec::new(),
        list => list.split(',').map(str::trim).collect(),
    };
    if head.eq_ignore_ascii_case("steps") {
        return steps(&args).map(|e| (e, None));
    }
    if head.eq_ignore_ascii_case("cubic-bezier") {
        return cubic_bezier(&args).map(|e| (e, None));
    }
    if head.eq_ignore_ascii_case("spring") {
        return spring_named(&args).map(|e| (e, None));
    }
    let Some(e) = fixed(head) else {
        return Err(unknown(name));
    };
    parametric(head, e, &args).map(|note| (e, Some(note)))
}

/// A fixed name: a word, or a GSAP family with an optional kind.
fn fixed(name: &str) -> Option<E> {
    if let Some((_, e)) = WORDS.iter().find(|(w, _)| w.eq_ignore_ascii_case(name)) {
        return Some(*e);
    }
    let (family, kind) = match name.split_once('.') {
        Some((family, kind)) => (family, kind_index(kind)?),
        None => (name, 1),
    };
    FAMILIES
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(family))
        .map(|(_, eases)| eases[kind])
}

/// `in`, `out` or `inOut` (or GSAP 2's `easeIn`, `easeOut`, `easeInOut`) as an index into [`KINDS`].
fn kind_index(kind: &str) -> Option<usize> {
    let kind = match kind.get(..4) {
        Some(ease) if ease.eq_ignore_ascii_case("ease") => &kind[4..],
        _ => kind,
    };
    KINDS.iter().position(|k| k.eq_ignore_ascii_case(kind))
}

/// `back.out(2)` or `elastic.out(1, 0.3)`: the ease is read as its default, and the note says so.
fn parametric(head: &str, e: E, args: &[&str]) -> Result<String, String> {
    let (most, defaults) = match e {
        E::BackIn | E::BackOut | E::BackInOut => (1, "s = 1.70158"),
        E::ElasticIn | E::ElasticOut => (2, "amplitude 1, period 0.3"),
        E::ElasticInOut => (2, "amplitude 1, period 0.45"),
        _ => {
            return Err(format!(
                "{head} takes no settings. Of GSAP's eases only back and elastic do (Klipolo reads those as their \
                 defaults); for a curve of your own use cubic-bezier(x1, y1, x2, y2) or spring(stiffness, damping, mass)."
            ))
        }
    };
    let values = numbers(args)
        .filter(|v| v.len() <= most)
        .ok_or_else(|| {
            format!(
                "{head}(…) takes at most {most} number{} in GSAP, and Klipolo draws it with its defaults ({defaults}) \
                 anyway: write {}.",
                if most == 1 { "" } else { "s" },
                e.name()
            )
        })?;
    let shown: Vec<String> = values.iter().map(|v| exact(*v)).collect();
    Ok(format!(
        "Klipolo reads {head}({}) as {} ({defaults}); parametric GSAP eases use their defaults.",
        shown.join(", "),
        e.name()
    ))
}

/// CSS's `steps(n)` and `steps(n, end)` (`jump-end`): n jumps, each at the end of its step.
fn steps(args: &[&str]) -> Result<E, String> {
    let at_end = match args.get(1) {
        None => true,
        Some(jump) => jump.eq_ignore_ascii_case("end") || jump.eq_ignore_ascii_case("jump-end"),
    };
    if !at_end {
        return Err(
            "Klipolo's steps jump at the end of each step: write steps(n) or steps(n, end)."
                .to_owned(),
        );
    }
    let n = args
        .first()
        .filter(|_| args.len() <= 2)
        .and_then(|n| n.parse::<u32>().ok());
    match n {
        Some(n) if (1..=MAX_STEPS).contains(&n) => Ok(E::Steps { n }),
        _ => Err(format!(
            "steps(n) takes a whole number of steps from 1 to {MAX_STEPS}, then optionally end."
        )),
    }
}

/// CSS's `cubic-bezier(x1, y1, x2, y2)`, within Klipolo's bounds.
fn cubic_bezier(args: &[&str]) -> Result<E, String> {
    let e = numbers(args).and_then(|v| match v[..] {
        [x1, y1, x2, y2] => Some(E::CubicBezier { x1, y1, x2, y2 }),
        _ => None,
    });
    e.filter(|e| e.is_valid()).ok_or_else(|| {
        format!(
            "cubic-bezier(x1, y1, x2, y2) takes four numbers: x1 and x2 from 0 to 1, y1 and y2 from {} to {}.",
            exact(EASE_Y.0),
            exact(EASE_Y.1)
        )
    })
}

/// `spring(stiffness, damping, mass)`: in range, and damped enough to settle.
fn spring_named(args: &[&str]) -> Result<E, String> {
    let range = |(lo, hi): (f64, f64)| format!("{}–{}", exact(lo), exact(hi));
    let Some([k, c, m]) = numbers(args).and_then(|v| <[f64; 3]>::try_from(v).ok()) else {
        return Err(format!(
            "spring(stiffness, damping, mass) takes three numbers: stiffness {}, damping {} and mass {}.",
            range(STIFFNESS),
            range(DAMPING),
            range(MASS)
        ));
    };
    let e = E::Spring {
        stiffness: k,
        damping: c,
        mass: m,
    };
    if e.is_valid() {
        return Ok(e);
    }
    if !spring::in_range(k, c, m) {
        return Err(format!(
            "{} is out of range: stiffness {}, damping {} and mass {}.",
            e.name(),
            range(STIFFNESS),
            range(DAMPING),
            range(MASS)
        ));
    }
    // Too little damping: name the least that would do, rounded up to a thousandth so it does.
    let mut thousandths = (spring::least_damping(k, m) * 1000.0).ceil();
    if !spring::is_valid(k, thousandths / 1000.0, m) {
        thousandths += 1.0;
    }
    Err(format!(
        "{} barely settles: its damping ratio, damping / (2 × √(stiffness × mass)), is {:.3}, and Klipolo needs at \
         least {}. Use a damping of at least {} with this stiffness and mass.",
        e.name(),
        spring::ratio(k, c, m),
        exact(MIN_SPRING_ZETA),
        exact(thousandths / 1000.0)
    ))
}

/// Each argument as a finite number, or None.
fn numbers(args: &[&str]) -> Option<Vec<f64>> {
    args.iter()
        .map(|a| a.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// `v` as the shortest text that reads back as the same number: `120`, `0.2`, `1e-7`.
fn exact(v: f64) -> String {
    let s = format!("{v:?}");
    match s.strip_suffix(".0") {
        Some(whole) => whole.to_owned(),
        None => s,
    }
}

fn unknown(name: &str) -> String {
    let shown: String = name.chars().take(40).collect();
    format!(
        "Klipolo doesn't know the ease {shown:?}. It reads GSAP's names (none; power0–power4, sine, expo, circ, back, \
         elastic and bounce, each with .in, .out or .inOut), CSS's (linear, ease, ease-in, ease-out, ease-in-out, \
         steps(n), cubic-bezier(x1, y1, x2, y2)), spring(stiffness, damping, mass) and hold."
    )
}
