//! Taints (for the costly settings): which settings (`params.<key>`) and arguments reach a value, and whether the
//! rises or falls with each. Conservative: where the direction isn't certain
//! the sign is unknown, so the setting is measured at both ends (`expr` has the rule per expression).

use naga::Handle;

use super::End;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    /// A member of `params`, by index.
    Param(u32),
    /// An argument of the function, by index.
    Arg(u32),
}

/// How a value moves with its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sign {
    Up,
    Down,
    Unknown,
}

impl Sign {
    pub(super) fn flip(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Unknown => Self::Unknown,
        }
    }

    /// How a value that moves `self` with an input moves with that input's source, when the input moves `inner`.
    pub(super) fn of(self, inner: Sign) -> Sign {
        match (self, inner) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (a, b) if a == b => Self::Up,
            _ => Self::Down,
        }
    }
}

/// Sources and signs, sorted by source.
pub(super) type Taint = Vec<(Source, Sign)>;

pub(super) fn add(t: &mut Taint, source: Source, sign: Sign) -> bool {
    match t.binary_search_by_key(&source, |&(s, _)| s) {
        Ok(i) if t[i].1 != sign && t[i].1 != Sign::Unknown => {
            t[i].1 = Sign::Unknown;
            true
        }
        Ok(_) => false,
        Err(i) => {
            t.insert(i, (source, sign));
            true
        }
    }
}

pub(super) fn join(t: &mut Taint, from: &[(Source, Sign)], map: fn(Sign) -> Sign) -> bool {
    from.iter()
        .fold(false, |changed, &(s, g)| add(t, s, map(g)) | changed)
}

pub(super) fn keep(s: Sign) -> Sign {
    s
}

pub(super) fn unknown(_: Sign) -> Sign {
    Sign::Unknown
}

/// A callee's source as the caller sees it: a setting as itself, an argument as the sources of the value passed.
pub(super) fn at_call<'a>(
    source: Source,
    arguments: &[Handle<naga::Expression>],
    expr: &'a [Taint],
) -> impl Iterator<Item = (Source, Sign)> + 'a {
    let own = matches!(source, Source::Param(_)).then_some((source, Sign::Up));
    let passed = match source {
        Source::Param(_) => &[][..],
        Source::Arg(j) => arguments
            .get(j as usize)
            .map_or(&[][..], |a| expr[a.index()].as_slice()),
    };
    own.into_iter().chain(passed.iter().copied())
}

/// Adds `(s, v)` unless `s` has an entry already: one per source, sorted by source.
pub(super) fn first<T>(list: &mut Vec<(Source, T)>, s: Source, v: T) {
    if let Err(i) = list.binary_search_by(|e| e.0.cmp(&s)) {
        list.insert(i, (s, v));
    }
}

/// An end found for an input, seen through a value that moves `sign` with the input's source. `Probe` is only ever set
/// on a colour or point at the end (`settings`), and stays as it is.
pub(super) fn through(end: End, sign: Sign) -> End {
    match (end, sign) {
        (End::Probe, _) => End::Probe,
        (_, Sign::Unknown) | (End::Both, _) => End::Both,
        (e, Sign::Up) => e,
        (End::Max, Sign::Down) => End::Min,
        (End::Min, Sign::Down) => End::Max,
    }
}
