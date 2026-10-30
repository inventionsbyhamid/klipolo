//! Closing a page (its keyword, its final period, the earliest it may end) and finishing a format's pages (spec
//! `PageBuilder`'s other half.

use super::keywords::{Keyword, KEYWORD_GAP_US};
use super::page_builder::{ms, Cand, PageBuilder};
use super::pages::Page;
use crate::time::Us;

impl PageBuilder<'_> {
    /// Ends the open page: its keyword, its last word's final period (`DropFinalPeriod`), and the earliest it may end.
    pub(super) fn close(&mut self) {
        let Some(o) = self.open.take() else {
            return;
        };
        let (from, to) = (o.first_word, self.keyword_of.len() as u32);
        let (start_us, end_us, hold) = if o.visible > 0 {
            let mut hold = o.start_us + ms(self.rules.min_page_ms);
            if let Some(cps) = self.rules.reading_cps.filter(|&c| c > 0.0) {
                hold = hold.max(o.start_us + (o.cols as f64 / cps * 1e6).round() as Us);
            }
            (o.start_us, o.end_us, Some(hold))
        } else {
            let at = self.cands.first().map_or(o.start_us, |c| c.start_us);
            (at, at, None)
        };
        let keyword = self.keywords && o.visible > 0 && self.pick_keyword(from as usize);
        let period_dropped = self.drop_final_period
            && self
                .cands
                .iter()
                .rev()
                .find(|c| !c.hidden)
                .is_some_and(|c| c.period);
        self.pages.push(Page {
            start_us,
            end_us,
            words: (from, to),
            index: self.pages.len() as u32,
            keyword,
            period_dropped,
        });
        self.holds.push((hold, o.cut_us));
        self.cands.clear();
    }

    /// The open page's keyword (its words from `from`): every word the person emphasised, else the best candidate
    /// (lowest priority, then first) at least `KEYWORD_GAP_US` after the last keyword, never one the person said isn't.
    fn pick_keyword(&mut self, from: usize) -> bool {
        let keywords = &mut self.keyword_of[from..];
        let forced = |c: &Cand| !c.hidden && c.emphasis == Some(true);
        if self.cands.iter().any(forced) {
            for (k, c) in keywords.iter_mut().zip(&self.cands) {
                if forced(c) {
                    *k = Some(c.best.map_or(Keyword::Plain, |b| b.1));
                    self.last_keyword_us = Some(c.start_us);
                }
            }
            return true;
        }
        let best = self
            .cands
            .iter()
            .enumerate()
            .filter(|(_, c)| c.emphasis != Some(false))
            .filter_map(|(i, c)| c.best.map(|(priority, k)| (priority, i, k)))
            .min_by_key(|&(priority, i, _)| (priority, i));
        match best {
            Some((_, i, k))
                if self
                    .last_keyword_us
                    .is_none_or(|last| self.cands[i].start_us - last >= KEYWORD_GAP_US) =>
            {
                keywords[i] = Some(k);
                self.last_keyword_us = Some(self.cands[i].start_us);
                true
            }
            _ => false,
        }
    }

    /// The pages and their keywords. Each page ends at `max(last word + min(lingerMs, gap), its hold)`, never past the
    /// next visible page's start, nor past the cut its span ends at (it never hangs over the next shot), unless the
    /// next page starts within `lingerMs` after that cut: then it holds to it, so a jump cut never blinks. A page of
    /// hidden words only takes no time, no earlier than the end of the page before it.
    pub(super) fn finish(mut self) -> (Vec<Page>, Vec<Option<Keyword>>) {
        self.close();
        let linger = ms(self.rules.linger_ms);
        let mut next: Option<Us> = None;
        for (p, &(hold, cut)) in self.pages.iter_mut().zip(&self.holds).rev() {
            if let Some(hold) = hold {
                let mut end = (p.end_us + linger).max(hold);
                end = next.map_or(end, |n| end.min(n));
                if end > cut {
                    end = next.filter(|&n| n - cut <= linger).unwrap_or(cut);
                }
                p.end_us = end.max(p.start_us);
                next = Some(p.start_us);
            }
        }
        let mut before: Option<Us> = None;
        for (p, (hold, _)) in self.pages.iter_mut().zip(&self.holds) {
            match hold {
                Some(_) => before = Some(p.end_us),
                None => {
                    let at = before.map_or(p.start_us, |b| p.start_us.max(b));
                    (p.start_us, p.end_us) = (at, at);
                }
            }
        }
        (self.pages, self.keyword_of)
    }
}
