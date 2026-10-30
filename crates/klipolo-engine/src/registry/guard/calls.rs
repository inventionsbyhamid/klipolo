//! A call in the cost walk: the arguments passed by value, the callee's cost, and its summary mapped through the
//! arguments at the call's place: its costly settings, its pointers, and its work tally each time the loops around the
//! call run. Also the walk's records of the tally and of where each source decides work.

use naga::{Function, Handle, Span};

use super::cost::Walk;
use super::flow::Ex;
use super::taint::{Sign, Source, at_call, first, through};
use super::tally::{self, Where, place};
use super::work::{Cost, HUGE, Work};

impl Walk<'_> {
    pub(super) fn call(
        &mut self,
        c: &mut Cost,
        function: Handle<Function>,
        arguments: &[Ex],
        mult: f64,
        span: Span,
    ) {
        let passed: f64 = arguments.iter().map(|a| self.weights.values(*a)).sum();
        c.add_straight(Work {
            ops: (1.0 + passed).min(HUGE),
            samples: 0.0,
        });
        self.sum.calls.push((function, mult));
        let sums = self.cx.sums;
        let Some(s) = sums[function.index()].as_ref() else {
            return;
        };
        c.merge(Cost {
            bound: s.bound,
            straight: s.straight,
            max_own: s.max_own,
            work: s.has_work,
        });
        self.sum.animates |= s.animates;
        self.sum.reads_layer |= s.reads_layer;
        self.sum.whole_params |= s.whole_params;
        let flow = self.flow;
        let expr = &flow.expr;
        for &(source, helper) in &s.via_pointer {
            for (s2, _) in at_call(source, arguments, expr) {
                first(&mut self.sum.via_pointer, s2, helper);
            }
        }
        for &(source, end) in &s.costly {
            for (s2, g) in at_call(source, arguments, expr) {
                self.sum.costly.push((s2, through(end, g)));
            }
            self.mark(
                at_call(source, arguments, expr).map(|(s2, _)| s2),
                HUGE,
                span,
            );
        }
        // The callee's tally, each time the loops around the call run.
        for &(source, w) in &s.decides {
            for (s2, _) in at_call(source, arguments, expr) {
                tally::add(&mut self.sum.decides, s2, w * mult);
            }
            self.mark(
                at_call(source, arguments, expr).map(|(s2, _)| s2),
                w * mult,
                span,
            );
        }
    }

    /// Each source of `taint` decides `w` more operations at `span` (the work tally).
    pub(super) fn decide(&mut self, taint: &[(Source, Sign)], w: f64, span: Span) {
        for &(s, _) in taint {
            tally::add(&mut self.sum.decides, s, w);
        }
        self.mark(taint.iter().map(|&(s, _)| s), w, span);
    }

    /// Records `span` as where each source decides `weight` operations, when that is the most so far and `span` is in
    /// the body.
    pub(super) fn mark(&mut self, sources: impl Iterator<Item = Source>, weight: f64, span: Span) {
        let Some(offset) = self.p.offset(span) else {
            return;
        };
        for s in sources {
            place(&mut self.sum.at, s, Where { weight, offset });
        }
    }
}
