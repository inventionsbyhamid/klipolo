//! The rules a caption style file must keep, checked when it loads: built-ins in their test and
//! at start, packs' when installed. The first broken rule is the answer, naming the JSON field.

use super::style::{hex_ok, ActiveDef, BoxDef, CaptionStyleDef, GlowDef, Paint, PerFormat};
use super::styles::STYLE_SCHEMA;
use crate::font::{font_ok, FontRef};

pub(super) type Checked = Result<(), String>;

pub(super) fn rule(ok: bool, field: &str, what: &str) -> Checked {
    if ok {
        Ok(())
    } else {
        Err(format!("{field}: {what}"))
    }
}

fn within(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && (lo..=hi).contains(&v)
}

pub(super) fn range(v: f64, lo: f64, hi: f64, field: &str) -> Checked {
    rule(within(v, lo, hi), field, &format!("must be {lo} to {hi}"))
}

pub(super) fn ms(v: u32, hi: u32, field: &str) -> Checked {
    rule(v <= hi, field, &format!("must be at most {hi} ms"))
}

fn per_format(p: &PerFormat<f64>, lo: f64, hi: f64, field: &str) -> Checked {
    rule(
        p.values().all(|&v| within(v, lo, hi)),
        field,
        &format!("must be {lo} to {hi} in every format"),
    )
}

impl CaptionStyleDef {
    /// The first rule this style breaks, as "field: what"; Ok when it keeps them all.
    pub fn check(&self) -> Result<(), String> {
        rule(
            self.schema <= STYLE_SCHEMA,
            "schema",
            "this style needs a newer Klipolo",
        )?;
        rule(
            self.schema == STYLE_SCHEMA,
            "schema",
            &format!("must be {STYLE_SCHEMA}"),
        )?;
        self.check_identity()?;
        self.check_type()?;
        self.check_colours()?;
        self.check_layers()?;
        self.check_pages()?;
        self.enter.check()?;
        self.shake.as_ref().map_or(Ok(()), |s| s.check())?;
        self.exit.check()?;
        self.check_variants()
    }

    fn check_identity(&self) -> Checked {
        let local = self.id.as_str().split_once('/').map_or("", |(_, l)| l);
        rule(
            self.id.is_well_formed() && local.starts_with("caption."),
            "id",
            "must be <plugin>/caption.<name>",
        )?;
        let n = self.name.chars().count();
        rule((1..=40).contains(&n), "name", "must be 1 to 40 characters")
    }

    fn check_type(&self) -> Checked {
        rule(
            font_ok(&FontRef::bundled(&self.font.family)),
            "font.family",
            "must be a font's name",
        )?;
        rule(
            (100..=900).contains(&self.font.weight),
            "font.weight",
            "must be 100 to 900",
        )?;
        for f in &self.fallbacks {
            rule(
                f.script.len() == 4 && f.script.bytes().all(|b| b.is_ascii_alphabetic()),
                "fallbacks.script",
                "must be a 4-letter script code",
            )?;
            rule(
                font_ok(&FontRef::bundled(&f.family)) && (100..=900).contains(&f.weight),
                "fallbacks",
                "must name a font and a weight of 100 to 900",
            )?;
        }
        per_format(&self.size, 8.0, 400.0, "size")?;
        range(self.autofit_min, 0.5, 1.0, "autofitMin")?;
        range(self.line_height, 0.8, 2.0, "lineHeight")?;
        range(self.char_em, 0.2, 1.2, "charEm")?;
        range(self.letter_spacing_em, -0.2, 0.5, "letterSpacingEm")?;
        ms(self.lead_ms, 200, "leadMs")?;
        range(self.tilt_deg, 0.0, 15.0, "tiltDeg")
    }

    /// A paint is hex, `accent`, or `contrast` where words can sit on a box (`fill`, `boxes.text`).
    fn paint(&self, p: &Paint, field: &str, on_box: bool) -> Checked {
        let ok = match p.0.as_str() {
            "accent" => true,
            "contrast" => on_box && self.boxes.is_some(),
            hex => hex_ok(hex),
        };
        rule(ok, field, "must be #RRGGBB, accent, or contrast on a box")
    }

    fn check_colours(&self) -> Checked {
        self.paint(&self.fill, "fill", true)?;
        rule(hex_ok(&self.accent), "accent", "must be #RRGGBB")?;
        rule(
            self.swatches.len() <= 16 && self.swatches.iter().all(|s| hex_ok(s)),
            "swatches",
            "must be at most 16 colours, each #RRGGBB",
        )?;
        if let Some(k) = &self.keyword {
            self.paint(&k.color, "keyword.color", false)?;
            rule(
                hex_ok(&k.gain) && hex_ok(&k.loss),
                "keyword.gain",
                "gain and loss must be #RRGGBB",
            )?;
        }
        if let Some(c) = &self.active.color {
            self.paint(&c.color, "active.color", false)?;
            ms(c.fade_ms, 1000, "active.color.fadeMs")?;
        }
        Ok(())
    }

    fn check_box(&self, b: &BoxDef, field: &str) -> Checked {
        self.paint(&b.color, field, false)?;
        if let Some(t) = &b.text {
            self.paint(t, field, true)?;
        }
        rule(
            within(b.pad_x_em, 0.0, 1.0) && within(b.pad_y_em, 0.0, 1.0),
            field,
            "pads must be 0 to 1 em",
        )?;
        range(b.radius_em, 0.0, 1.0, field)?;
        range(b.alpha, 0.0, 1.0, field)?;
        rule(
            b.glide_ms <= 500 && b.grow_ms <= 500,
            field,
            "glide and grow must be at most 500 ms",
        )
    }

    fn check_glow(&self, g: &GlowDef, field: &str) -> Checked {
        if let Some(c) = &g.color {
            self.paint(c, field, false)?;
        }
        rule(
            within(g.blur_em, 0.0, 1.0) && within(g.alpha, 0.0, 1.0),
            field,
            "blur must be 0 to 1 em and alpha 0 to 1",
        )
    }

    fn check_layers(&self) -> Checked {
        if let Some(o) = &self.outline {
            self.paint(&o.color, "outline.color", false)?;
            range(o.width_em, 0.0, 0.3, "outline.widthEm")?;
        }
        if let Some(s) = &self.shadow {
            self.paint(&s.color, "shadow.color", false)?;
            range(s.alpha, 0.0, 1.0, "shadow.alpha")?;
            range(s.blur_em, 0.0, 1.0, "shadow.blurEm")?;
            rule(
                within(s.dx_em, -0.5, 0.5) && within(s.dy_em, -0.5, 0.5),
                "shadow.dxEm",
                "offsets must be within ±0.5 em",
            )?;
        }
        if let Some(b) = &self.boxes {
            self.check_box(b, "boxes")?;
        }
        let ActiveDef {
            color: _,
            scale,
            glow,
            dim_upcoming,
        } = &self.active;
        if let Some(g) = glow {
            self.check_glow(g, "active.glow")?;
        }
        rule(
            scale.is_none_or(|s| within(s, 1.0, 1.5)),
            "active.scale",
            "must be 1 to 1.5",
        )?;
        rule(
            dim_upcoming.is_none_or(|d| within(d, 0.0, 1.0)),
            "active.dimUpcoming",
            "must be 0 to 1",
        )?;
        if let Some(k) = &self.keyword {
            range(k.scale, 1.0, 1.5, "keyword.scale")?;
            if let Some(g) = &k.glow {
                self.check_glow(g, "keyword.glow")?;
            }
        }
        Ok(())
    }

    fn check_pages(&self) -> Checked {
        let p = &self.pages;
        rule(
            p.max_words.is_none_or(|w| (1..=24).contains(&w)),
            "pages.maxWords",
            "must be 1 to 24",
        )?;
        rule(
            (1..=4).contains(&p.max_lines),
            "pages.maxLines",
            "must be 1 to 4",
        )?;
        per_format(&p.max_line_width, 0.2, 1.0, "pages.maxLineWidth")?;
        rule(
            p.max_chars
                .as_ref()
                .is_none_or(|c| c.values().all(|n| (8..=80).contains(n))),
            "pages.maxChars",
            "must be 8 to 80",
        )?;
        ms(p.break_silence_ms, 5_000, "pages.breakSilenceMs")?;
        rule(
            p.min_page_ms <= p.max_page_ms && p.max_page_ms <= 10_000,
            "pages.maxPageMs",
            "must be at least minPageMs and at most 10000 ms",
        )?;
        ms(p.linger_ms, 2_000, "pages.lingerMs")?;
        rule(
            p.reading_cps.is_none_or(|c| within(c, 5.0, 40.0)),
            "pages.readingCps",
            "must be 5 to 40",
        )?;
        rule(
            self.position.values().all(|p| within(p.y, 0.05, 0.95)),
            "position.y",
            "must be 0.05 to 0.95 in every format",
        )
    }

    fn check_variants(&self) -> Checked {
        for (i, v) in self.variants.iter().enumerate() {
            rule(
                (1..=16).contains(&v.key.len()) && v.key.bytes().all(|b| b.is_ascii_lowercase()),
                "variants.key",
                "must be 1 to 16 letters a–z",
            )?;
            rule(
                !self.variants[..i].iter().any(|w| w.key == v.key),
                "variants.key",
                "must be unique",
            )?;
            let n = v.name.chars().count();
            rule(
                (1..=24).contains(&n),
                "variants.name",
                "must be 1 to 24 characters",
            )?;
            if let Some(a) = &v.accent {
                rule(hex_ok(a), "variants.accent", "must be #RRGGBB")?;
            }
            if let Some(f) = &v.fill {
                self.paint(f, "variants.fill", true)?;
            }
            if let Some(o) = &v.outline {
                self.paint(&o.color, "variants.outline", false)?;
                range(o.width_em, 0.0, 0.3, "variants.outline")?;
            }
            if let Some(b) = &v.boxes {
                // A variant's box may bring `contrast` text with it, so it is checked as the box it makes.
                let with_box = CaptionStyleDef {
                    boxes: Some(b.clone()),
                    ..self.clone()
                };
                with_box.check_box(b, "variants.boxes")?;
            }
            if let Some(s) = &v.shadow {
                self.paint(&s.color, "variants.shadow", false)?;
                range(s.alpha, 0.0, 1.0, "variants.shadow")?;
            }
        }
        Ok(())
    }
}
