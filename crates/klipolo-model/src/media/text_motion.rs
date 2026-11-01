//! Animated words: words that come in letter by letter, word by word or line by line, each unit from its own values,
//! as one `scene@1` motion graphic made from a few settings. The layer is a generator of [`SOURCE`] whose settings are
//! the words, their look and the animation ([`params`]); the scene is made from those settings alone ([`scene`]), so a
//! project file can only ever ask for a scene of this one shape, held to [`LIMITS`] and [`MAX_LETTERS`], and the engine
//! makes the same scene from the same settings. Its card shows and edits every setting, as for any generator.
//!
//! The scene is one text node filling the layer's box, split into units (`split`), with one `from` tween over the
//! units from the values given (each property at rest is left out) and, when asked, a `to` tween back to them in the
//! exit segment. Its entrance is the `in` segment and its exit the `out` one, so the hold between stretches to the
//! layer's length. The look follows a text layer's: its font, size, weight, italic, colour, alignment, outline, the box
//! behind the words (a span's background) and the drop shadow the engine draws for a text layer.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use crate::contrib::ContribId;
use crate::font::FontRef;
use crate::motion::Ease;
use crate::params::{ParamKind, ParamSpec, ParamUnit, ParamValue};
use crate::scene::{
    Align, Node, NodeType, Outline, Props, Scene, SceneFont, Segments, Shadow, Span,
    SpanBackground, Split, StaggerArg, StaggerFrom, Tween, Val, SCENE_VERSION,
};
use crate::timeline::{Outline as TextOutline, TextAlign, TextStyle};

/// The generator these layers are (`klipolo.builtin/scene.text-motion`).
pub const SOURCE: &str = "klipolo.builtin/scene.text-motion";

/// What one unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Letter,
    Word,
    Line,
}

/// Which unit starts first: the first, the last, those in the middle, or in a shuffled order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Start,
    End,
    Center,
    Random,
}

/// Where each unit comes in from; None leaves a property at rest (it doesn't move).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct From {
    /// 1080p pixels below its place (negative: above).
    pub y: Option<f64>,
    /// 1 is its own size.
    pub scale: Option<f64>,
    pub opacity: Option<f64>,
    /// Degrees clockwise.
    pub rotation: Option<f64>,
    /// 1080p pixels.
    pub blur: Option<f64>,
}

impl From {
    /// Whether every property is at rest (or unset): nothing would move.
    pub fn is_rest(&self) -> bool {
        self.values().is_empty()
    }

    /// The scene properties that move, with where they start: those not at rest, in a fixed order.
    fn values(&self) -> Vec<(&'static str, Val)> {
        let mut out = Vec::new();
        if let Some(y) = self.y.filter(|v| *v != 0.0) {
            out.push(("y", Val::Str(format!("{}px", exact(y)))));
        }
        for (name, value, rest) in [
            ("scale", self.scale, 1.0),
            ("opacity", self.opacity, 1.0),
            ("rotate", self.rotation, 0.0),
            ("blur", self.blur, 0.0),
        ] {
            if let Some(v) = value.filter(|v| *v != rest) {
                out.push((name, Val::Num(v)));
            }
        }
        out
    }
}

/// The values each `From` property takes, in its order (y, scale, opacity, rotation, blur).
pub const LIMITS: [(f64, f64); 5] = [
    (-1080.0, 1080.0),
    (0.0, 10.0),
    (0.0, 1.0),
    (-720.0, 720.0),
    (0.0, 40.0),
];
/// How long each unit takes, in seconds.
pub const UNIT_SECS: RangeInclusive<f64> = 0.05..=5.0;
/// The time between one unit and the next, in seconds.
pub const STAGGER_SECS: RangeInclusive<f64> = 0.0..=2.0;
/// The most characters the words may have.
pub const MAX_CHARS: usize = 200;
/// The most letters that come in one by one: past it, Klipolo can't draw every letter's frame in time, so a longer text
/// comes in by word or line.
pub const MAX_LETTERS: usize = 80;
/// The longest entrance (or exit), in seconds: a long text with a long stagger goes by word or faster.
pub const MAX_ENTRANCE_SECS: f64 = 20.0;
/// What a unit takes, and the time between units, when the person said nothing.
pub const DEFAULT_UNIT_SECS: f64 = 0.5;

impl Unit {
    /// The time between units when none is said: quick for letters, slower for words and lines.
    pub fn default_stagger(self) -> f64 {
        match self {
            Unit::Letter => 0.04,
            Unit::Word => 0.12,
            Unit::Line => 0.25,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Unit::Letter => "letter",
            Unit::Word => "word",
            Unit::Line => "line",
        }
    }

    fn split(self) -> Split {
        match self {
            Unit::Letter => Split::Chars,
            Unit::Word => Split::Words,
            Unit::Line => Split::Lines,
        }
    }

    /// How many units `text` has, as near as it can be told without laying it out (a line that wraps adds one).
    pub fn count(self, text: &str) -> usize {
        match self {
            Unit::Letter => text.chars().filter(|c| !c.is_whitespace()).count(),
            Unit::Word => text.split_whitespace().count(),
            Unit::Line => text.lines().filter(|l| !l.trim().is_empty()).count(),
        }
        .max(1)
    }
}

impl Order {
    fn word(self) -> &'static str {
        match self {
            Order::Start => "start",
            Order::End => "end",
            Order::Center => "center",
            Order::Random => "random",
        }
    }

    fn stagger_from(self) -> StaggerFrom {
        match self {
            Order::Start => StaggerFrom::Start,
            Order::End => StaggerFrom::End,
            Order::Center => StaggerFrom::Center,
            Order::Random => StaggerFrom::Random,
        }
    }
}

const UNITS: [Unit; 3] = [Unit::Letter, Unit::Word, Unit::Line];
const ORDERS: [Order; 4] = [Order::Start, Order::End, Order::Center, Order::Random];

/// How the words come in (and go out): None takes the default.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMotion {
    pub unit: Unit,
    pub from: From,
    /// Each unit's time; None: [`DEFAULT_UNIT_SECS`].
    pub unit_secs: Option<f64>,
    /// None: power2's out.
    pub ease: Option<Ease>,
    /// None: the unit's [`Unit::default_stagger`].
    pub stagger_secs: Option<f64>,
    /// None: from the start.
    pub order: Option<Order>,
    /// They leave the way they came, in the exit segment.
    pub exit: bool,
}

impl TextMotion {
    fn unit_secs(&self) -> f64 {
        self.unit_secs.unwrap_or(DEFAULT_UNIT_SECS)
    }

    fn stagger_secs(&self) -> f64 {
        self.stagger_secs
            .unwrap_or_else(|| self.unit.default_stagger())
    }

    /// How long every unit of `text` takes to come in: the last unit's start and its own time.
    pub fn entrance_secs(&self, text: &str) -> f64 {
        let n = self.unit.count(text) as f64;
        self.stagger_secs() * (n - 1.0) + self.unit_secs()
    }
}

/// Why `motion` can't show `text`, in a sentence for the agent that says what to do instead; None when it can.
pub fn problem(text: &str, motion: &TextMotion) -> Option<String> {
    let chars = text.chars().count();
    if chars > MAX_CHARS {
        return Some(format!(
            "the words are {chars} characters; animated words hold at most {MAX_CHARS}."
        ));
    }
    let letters = Unit::Letter.count(text);
    if motion.unit == Unit::Letter && letters > MAX_LETTERS {
        return Some(format!(
            "{letters} letters can't each come in on their own in time; at most {MAX_LETTERS} can. Animate by word \
             instead."
        ));
    }
    let secs = motion.entrance_secs(text);
    if secs > MAX_ENTRANCE_SECS {
        return Some(format!(
            "the words would take {} s to come in, past the most, {MAX_ENTRANCE_SECS} s. Use a shorter stagger or \
             time.",
            exact((secs * 10.0).round() / 10.0)
        ));
    }
    None
}

/// A number as JSON writes it, without a trailing `.0` noise: `60`, `0.5`.
fn exact(v: f64) -> String {
    format!("{}", (v * 1000.0).round() / 1000.0 + 0.0)
}

fn align(a: TextAlign) -> Align {
    match a {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    }
}

/// `motion` showing `text` in `style`, as a scene: total for any input [`problem`] lets through.
pub fn scene(text: &str, style: &TextStyle, motion: &TextMotion) -> Scene {
    let size = style.size;
    let mut props = Props {
        font: Some(SceneFont::Ref {
            font: style.font.clone(),
            weight: Some(style.weight),
            italic: Some(style.italic),
        }),
        size: Some(Val::Num(size)),
        fill: Some(crate::scene::Paint::Color(style.color.clone())),
        align: Some(align(style.align)),
        split: Some(motion.unit.split()),
        fit: Some(crate::scene::Fit::Shrink),
        ..Props::default()
    };
    match &style.background {
        // The box behind the words, as a text layer draws it: 0.2 em round them with 0.15 em corners.
        Some(color) => {
            props.spans = Some(vec![Span {
                text: text.to_owned(),
                fill: None,
                size: None,
                weight: None,
                italic: None,
                background: Some(SpanBackground {
                    color: color.clone(),
                    padding: Some(size * 0.2),
                    radius: Some(size * 0.15),
                }),
            }]);
        }
        None => props.text = Some(text.to_owned()),
    }
    if let Some(o) = &style.outline {
        props.outline = Some(Outline {
            width: Val::Num(o.width),
            color: o.color.clone(),
        });
    }
    if style.shadow {
        // A text layer's shadow: one hard copy 0.02 em right and 0.055 em down, black at 0.35 x 0.85.
        props.shadow = Some(Shadow {
            x: Some(Val::Num(size * 0.02)),
            y: Some(Val::Num(size * 0.055)),
            blur: Some(Val::Num(0.0)),
            spread: None,
            color: Some("#0000004C".to_owned()),
        });
    }
    let node = Node {
        kind: NodeType::Text,
        id: Some("words".to_owned()),
        class: None,
        bounds: None,
        props,
        animate: BTreeMap::new(),
        children: Vec::new(),
    };
    let stagger = StaggerArg::Spec {
        each: Some(motion.stagger_secs()),
        amount: None,
        from: Some(motion.order.unwrap_or(Order::Start).stagger_from()),
        seed: None,
    };
    let values: BTreeMap<String, Val> = (motion.from.values().into_iter())
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let unit = motion.unit_secs();
    let ease = motion.ease.unwrap_or(Ease::Power2Out);
    let mut timeline = vec![Tween {
        from: Some(crate::scene::Selectors::One("#words".to_owned())),
        at: Some(Val::Num(0.0)),
        dur: Some(unit),
        ease: Some(ease.name()),
        stagger: Some(stagger.clone()),
        vars: values.clone(),
        ..Tween::default()
    }];
    let enter = motion.entrance_secs(text);
    let out = if motion.exit { enter } else { 0.0 };
    if motion.exit {
        timeline.push(Tween {
            to: Some(crate::scene::Selectors::One("#words".to_owned())),
            at: Some(Val::Str("out".to_owned())),
            dur: Some(unit),
            ease: Some(Ease::Power2In.name()),
            stagger: Some(stagger),
            vars: values,
            ..Tween::default()
        });
    }
    // A short hold between, which the layer's length stretches.
    let hold = 0.5;
    Scene {
        scene: SCENE_VERSION,
        duration: enter + hold + out,
        segments: Some(Segments { enter, out }),
        params: Vec::new(),
        nodes: vec![node],
        timeline,
    }
}

/// The settings a layer of these words in `style` moving as `motion` stores ([`SOURCE`]'s, [`definition_params`]).
pub fn params(text: &str, style: &TextStyle, motion: &TextMotion) -> BTreeMap<String, ParamValue> {
    let mut p = BTreeMap::new();
    let mut put = |k: &str, v: ParamValue| {
        p.insert(k.to_owned(), v);
    };
    put("title", ParamValue::Text(text.to_owned()));
    put("by", ParamValue::Text(motion.unit.word().to_owned()));
    let f = motion.from;
    for (key, value) in [
        ("fromY", f.y),
        ("fromScale", f.scale),
        ("fromOpacity", f.opacity),
        ("fromRotation", f.rotation),
        ("fromBlur", f.blur),
    ] {
        if let Some(v) = value {
            put(key, ParamValue::Number(v));
        }
    }
    put("sec", ParamValue::Number(motion.unit_secs()));
    put("stagger", ParamValue::Number(motion.stagger_secs()));
    put(
        "ease",
        ParamValue::Text(motion.ease.unwrap_or(Ease::Power2Out).name()),
    );
    put(
        "order",
        ParamValue::Text(motion.order.unwrap_or(Order::Start).word().to_owned()),
    );
    put("exit", ParamValue::Bool(motion.exit));
    put("font", ParamValue::Font(style.font.clone()));
    put("weight", ParamValue::Number(f64::from(style.weight)));
    put("italic", ParamValue::Bool(style.italic));
    put("size", ParamValue::Number(style.size));
    put("color", ParamValue::Text(style.color.clone()));
    if let Some(bg) = &style.background {
        put("highlight", ParamValue::Text(bg.clone()));
    }
    if let Some(o) = &style.outline {
        put("outline", ParamValue::Text(o.color.clone()));
        put("outlineWidth", ParamValue::Number(o.width));
    }
    put("shadow", ParamValue::Bool(style.shadow));
    put(
        "align",
        ParamValue::Text(
            match style.align {
                TextAlign::Left => "left",
                TextAlign::Center => "center",
                TextAlign::Right => "right",
            }
            .to_owned(),
        ),
    );
    p
}

/// The words, their look and their motion a layer's settings say, any missing or unreadable one taking the
/// definition's default: total, so any stored settings make a scene.
pub fn read(params: &BTreeMap<String, ParamValue>) -> (String, TextStyle, TextMotion) {
    let text_of = |k: &str| match params.get(k) {
        Some(ParamValue::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    let number = |k: &str| match params.get(k) {
        Some(ParamValue::Number(n)) if n.is_finite() => Some(*n),
        _ => None,
    };
    let flag = |k: &str| match params.get(k) {
        Some(ParamValue::Bool(b)) => Some(*b),
        _ => None,
    };
    let within = |v: Option<f64>, (lo, hi): (f64, f64)| v.map(|v| v.clamp(lo, hi));
    let text: String = text_of("title")
        .unwrap_or("Your words")
        .chars()
        .take(MAX_CHARS)
        .collect();
    let unit = (UNITS.into_iter())
        .find(|u| Some(u.word()) == text_of("by"))
        .unwrap_or(Unit::Word);
    let [ly, ls, lo, lr, lb] = LIMITS;
    let from = From {
        y: within(number("fromY"), ly),
        scale: within(number("fromScale"), ls),
        opacity: within(number("fromOpacity"), lo),
        rotation: within(number("fromRotation"), lr),
        blur: within(number("fromBlur"), lb),
    };
    let secs = |k: &str, r: &RangeInclusive<f64>| number(k).map(|v| v.clamp(*r.start(), *r.end()));
    let ease = text_of("ease")
        .and_then(Ease::from_name)
        .map(|(e, _)| e)
        .filter(|e| e.is_valid());
    let motion = TextMotion {
        unit,
        from,
        unit_secs: secs("sec", &UNIT_SECS),
        ease,
        stagger_secs: secs("stagger", &STAGGER_SECS),
        order: (ORDERS.into_iter()).find(|o| Some(o.word()) == text_of("order")),
        exit: flag("exit").unwrap_or(false),
    };
    let base = TextStyle::default();
    let color = |k: &str| text_of(k).filter(|c| is_color(c)).map(str::to_owned);
    let style = TextStyle {
        font: match params.get("font") {
            Some(ParamValue::Font(f)) => f.clone(),
            _ => base.font.clone(),
        },
        size: within(number("size"), (4.0, 400.0)).unwrap_or(base.size),
        weight: number("weight")
            .map(|w| (w.round() as i64).clamp(100, 900) as u16)
            .unwrap_or(base.weight),
        italic: flag("italic").unwrap_or(false),
        color: color("color").unwrap_or(base.color.clone()),
        background: color("highlight"),
        outline: color("outline").map(|c| TextOutline {
            color: c,
            width: within(number("outlineWidth"), (0.0, 100.0)).unwrap_or(0.0),
        }),
        shadow: flag("shadow").unwrap_or(base.shadow),
        align: match text_of("align") {
            Some("left") => TextAlign::Left,
            Some("right") => TextAlign::Right,
            _ => TextAlign::Center,
        },
    };
    (text, style, motion)
}

/// Whether `c` is `#RRGGBB` or `#RRGGBBAA`.
fn is_color(c: &str) -> bool {
    let hex = c.strip_prefix('#').unwrap_or_default();
    matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The scene a layer of [`SOURCE`] with these settings draws: [`read`], then [`scene`] (a text past what [`problem`]
/// lets through comes in by word).
pub fn scene_of(params: &BTreeMap<String, ParamValue>) -> Scene {
    let (text, style, mut motion) = read(params);
    if problem(&text, &motion).is_some() {
        motion.unit = Unit::Word;
        motion.stagger_secs = motion
            .stagger_secs
            .map(|s| s.min(MAX_ENTRANCE_SECS / Unit::Word.count(&text) as f64 / 2.0));
    }
    scene(&text, &style, &motion)
}

fn spec(key: &str, label: &str, kind: ParamKind) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        label: label.to_owned(),
        visible_when: None,
        kind,
    }
}

fn float(key: &str, label: &str, (min, max): (f64, f64), default: f64, step: f64) -> ParamSpec {
    spec(
        key,
        label,
        ParamKind::Float {
            min,
            max,
            default,
            step: Some(step),
            unit: None,
        },
    )
}

fn choice(key: &str, label: &str, options: &[&str], default: &str) -> ParamSpec {
    spec(
        key,
        label,
        ParamKind::Choice {
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            default: default.to_owned(),
        },
    )
}

/// [`SOURCE`]'s settings, as its card shows them.
pub fn definition_params() -> Vec<ParamSpec> {
    let [ly, ls, lo, lr, lb] = LIMITS;
    let seconds = |key: &str, label: &str, (min, max): (f64, f64), default: f64| {
        let mut s = float(key, label, (min, max), default, 0.01);
        if let ParamKind::Float { unit, .. } = &mut s.kind {
            *unit = Some(ParamUnit::Seconds);
        }
        s
    };
    vec![
        spec(
            "title",
            "Words",
            ParamKind::Text {
                default: "Your words".to_owned(),
                max_len: MAX_CHARS as u32,
            },
        ),
        choice("by", "By", &UNITS.map(Unit::word), "word"),
        float("fromY", "From below", ly, 0.0, 1.0),
        float("fromScale", "From size", ls, 1.0, 0.05),
        float("fromOpacity", "From opacity", lo, 1.0, 0.05),
        float("fromRotation", "From turn", lr, 0.0, 1.0),
        float("fromBlur", "From blur", lb, 0.0, 1.0),
        seconds(
            "sec",
            "Each one",
            (*UNIT_SECS.start(), *UNIT_SECS.end()),
            DEFAULT_UNIT_SECS,
        ),
        seconds(
            "stagger",
            "Between",
            (*STAGGER_SECS.start(), *STAGGER_SECS.end()),
            Unit::Word.default_stagger(),
        ),
        spec(
            "ease",
            "Ease",
            ParamKind::Text {
                default: Ease::Power2Out.name(),
                max_len: crate::ease_names::MAX_NAME_LEN as u32,
            },
        ),
        choice("order", "First", &ORDERS.map(Order::word), "start"),
        spec(
            "exit",
            "Leave the same way",
            ParamKind::Bool { default: false },
        ),
        spec(
            "font",
            "Font",
            ParamKind::Font {
                default: FontRef::inter(),
            },
        ),
        float("weight", "Weight", (100.0, 900.0), 700.0, 100.0),
        spec("italic", "Italic", ParamKind::Bool { default: false }),
        float("size", "Size", (4.0, 400.0), 64.0, 1.0),
        spec(
            "color",
            "Colour",
            ParamKind::Color {
                default: "#FFFFFF".to_owned(),
                alpha: true,
            },
        ),
        spec(
            "highlight",
            "Highlight",
            ParamKind::Color {
                default: "#00000000".to_owned(),
                alpha: true,
            },
        ),
        spec(
            "outline",
            "Outline",
            ParamKind::Color {
                default: "#000000".to_owned(),
                alpha: true,
            },
        ),
        float("outlineWidth", "Outline width", (0.0, 100.0), 0.0, 0.5),
        spec("shadow", "Shadow", ParamKind::Bool { default: true }),
        choice("align", "Align", &["left", "center", "right"], "center"),
    ]
}

/// [`SOURCE`] as an id.
pub fn source() -> ContribId {
    ContribId::from(SOURCE)
}
