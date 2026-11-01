//! The eases scenes and layer motion share: the published equations, springs that settle into their key and never leave
//! the ease bounds, and GSAP's and CSS's names read and written.

use klipolo_model::ease_names;
use klipolo_model::motion::{spring_peak, Ease, EASE_Y};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn every_ease_starts_at_zero_and_ends_at_one() {
    for e in [
        Ease::SineIn,
        Ease::SineOut,
        Ease::SineInOut,
        Ease::ExpoIn,
        Ease::ExpoOut,
        Ease::ExpoInOut,
        Ease::CircIn,
        Ease::CircOut,
        Ease::CircInOut,
        Ease::Steps { n: 4 },
        Ease::Hold,
        Ease::Spring {
            stiffness: 170.0,
            damping: 26.0,
            mass: 1.0,
        },
        Ease::Spring {
            stiffness: 100.0,
            damping: 5.0,
            mass: 1.0,
        },
    ] {
        // Exactly, so a key's value is reached exactly (sine.in's cosine of π/2 isn't quite 0 in f64).
        assert_eq!(e.apply(0.0), 0.0, "{e:?} at 0");
        assert_eq!(e.apply(1.0), 1.0, "{e:?} at 1");
    }
}

#[test]
fn the_published_equations() {
    assert!(near(
        Ease::SineOut.apply(0.5),
        (std::f64::consts::PI / 4.0).sin()
    ));
    assert!(near(Ease::ExpoIn.apply(0.5), 2f64.powf(-5.0)));
    assert!(near(Ease::CircOut.apply(0.5), (1.0 - 0.25f64).sqrt()));
    assert_eq!(Ease::Steps { n: 4 }.apply(0.49), 0.25);
    assert_eq!(Ease::Hold.apply(0.999), 0.0);
    // An underdamped spring overshoots, then settles.
    let s = Ease::Spring {
        stiffness: 100.0,
        damping: 5.0,
        mass: 1.0,
    };
    assert!(
        (0..=100)
            .map(|i| s.apply(i as f64 / 100.0))
            .fold(0.0, f64::max)
            > 1.05
    );
}

#[test]
fn names_read_as_gsap_and_css_write_them() {
    assert_eq!(Ease::from_name("power3.out").unwrap().0, Ease::Power3Out);
    assert_eq!(Ease::from_name("expo.inOut").unwrap().0, Ease::ExpoInOut);
    assert_eq!(Ease::from_name("none").unwrap().0, Ease::Linear);
    assert_eq!(Ease::from_name("ease-in-out").unwrap().0, Ease::EaseInOut);
    assert_eq!(Ease::from_name("steps(6)").unwrap().0, Ease::Steps { n: 6 });
    assert_eq!(
        Ease::from_name("cubic-bezier(0.2, 0, 0, 1)").unwrap().0,
        Ease::CubicBezier {
            x1: 0.2,
            y1: 0.0,
            x2: 0.0,
            y2: 1.0
        }
    );
    let (e, note) = Ease::from_name("back.out(2)").unwrap();
    assert_eq!(e, Ease::BackOut);
    assert!(note.unwrap().contains("1.70158"));
    assert!(Ease::from_name("wobble.out").is_none());
    for e in [
        Ease::Power2InOut,
        Ease::Steps { n: 3 },
        Ease::Spring {
            stiffness: 120.0,
            damping: 14.0,
            mass: 1.0,
        },
    ] {
        assert_eq!(Ease::from_name(&e.name()).unwrap().0, e);
    }
}

#[test]
fn a_spring_needs_some_damping_and_never_leaves_the_ease_bounds() {
    // ζ = c / (2√(km)); below 0.05 the spring barely settles and its normalised curve explodes.
    assert!(!Ease::Spring {
        stiffness: 1000.0,
        damping: 0.0,
        mass: 10.0
    }
    .is_valid());
    assert!(!Ease::Spring {
        stiffness: 1000.0,
        damping: 3.0,
        mass: 10.0
    }
    .is_valid()); // ζ ≈ 0.015
    let (lo, hi) = EASE_Y;
    // ζ: 0.05 (the floor), 0.063, 5, 158, 0.159 — all valid.
    for (k, c, m) in [
        (1000.0, 10.0, 10.0),
        (1.0, 0.4, 10.0),
        (1000.0, 100.0, 0.1),
        (1.0, 100.0, 0.1),
        (500.0, 7.1, 1.0),
    ] {
        let e = Ease::Spring {
            stiffness: k,
            damping: c,
            mass: m,
        };
        assert!(e.is_valid(), "{e:?}");
        let mut top = f64::MIN;
        for i in 0..=1000 {
            let v = e.apply(i as f64 / 1000.0);
            assert!(v.is_finite() && v >= lo && v <= hi, "{e:?} at {i}: {v}");
            top = top.max(v);
        }
        // `spring_peak` bounds the curve (an effect target is sized for it), and closely.
        let peak = spring_peak(k, c, m);
        assert!(
            top <= peak + 1e-9 && peak - top < 0.01,
            "{e:?}: {top} vs {peak}"
        );
    }
    // An ease no one checked stays bounded too: the curve is drawn with the floor's damping at least.
    let wild = Ease::Spring {
        stiffness: 1000.0,
        damping: f64::NAN,
        mass: 0.0,
    };
    assert!(!wild.is_valid());
    assert!((0..=100)
        .map(|i| wild.apply(f64::from(i) / 100.0))
        .all(|v| v.is_finite() && v >= lo && v <= hi));
}

/// The key's span is the spring's settle time, also when it is overdamped (its slow decay sets it), so the motion
/// comes to rest at the key instead of stopping dead part of the way there.
#[test]
fn a_damped_spring_comes_to_rest_at_its_key() {
    // ζ: 1 (critical), 2, 158.
    for (k, c, m) in [(100.0, 20.0, 1.0), (100.0, 40.0, 1.0), (1.0, 100.0, 0.1)] {
        let e = Ease::Spring {
            stiffness: k,
            damping: c,
            mass: m,
        };
        let v: Vec<f64> = (0..=100).map(|i| e.apply(f64::from(i) / 100.0)).collect();
        assert!(v.windows(2).all(|w| w[1] >= w[0]), "{e:?} rises: {v:?}");
        assert!(1.0 - v[99] < 0.005, "{e:?} is still moving: {}", v[99]);
        assert_eq!(spring_peak(k, c, m), 1.0);
    }
}

#[test]
fn steps_and_springs_are_checked_where_they_come_in() {
    assert!(Ease::Steps { n: 1 }.is_valid() && Ease::Steps { n: 64 }.is_valid());
    assert!(!Ease::Steps { n: 0 }.is_valid() && !Ease::Steps { n: 65 }.is_valid());
    assert!(!Ease::Spring {
        stiffness: 1001.0,
        damping: 50.0,
        mass: 1.0
    }
    .is_valid());
    assert!(!Ease::Spring {
        stiffness: 100.0,
        damping: 50.0,
        mass: f64::INFINITY
    }
    .is_valid());
    assert_eq!(
        Ease::from_name("steps(4, end)").unwrap().0,
        Ease::Steps { n: 4 }
    );
    assert!(Ease::from_name("steps(4, start)").is_none());
    assert!(Ease::from_name("steps(65)").is_none());
    assert!(Ease::from_name("cubic-bezier(0.2, 5, 0, 1)").is_none());
    // An undamped spring is refused, and the reason names the least damping that would do: 0.1 × √(1000 × 10).
    assert!(Ease::from_name("spring(1000, 3, 10)").is_none());
    let why = ease_names::read("spring(1000, 3, 10)").unwrap_err();
    assert!(why.contains("at least 10 "), "{why}");
    let need = ease_names::read("spring(1, 0.1, 10)").unwrap_err();
    assert!(need.contains("at least 0.317 "), "{need}");
    assert!(Ease::from_name("spring(1, 0.317, 10)").is_some());
    assert!(ease_names::read("wobble.out")
        .unwrap_err()
        .contains("\"wobble.out\""));
    // Names are capped where they come in (this one would read as steps(4)).
    assert!(Ease::from_name(&format!("steps({}4)", "0".repeat(150))).is_none());
    // A refused steps or spring value loaded from a project reads right to the person.
    let (_, english) = klipolo_model::messages::MESSAGES
        .iter()
        .find(|(code, _)| *code == "model.motion.badEase")
        .unwrap();
    assert!(
        english.contains("steps") && english.contains("spring"),
        "{english}"
    );
}

#[test]
fn gsap_aliases_bare_families_and_parametric_forms_read_as_klipolo_has_them() {
    for (name, e) in [
        ("power2", Ease::Power2Out),
        ("Power2.easeOut", Ease::Power2Out),
        ("quad.in", Ease::Power1In),
        ("strong.inOut", Ease::Power4InOut),
        ("power0.in", Ease::Linear),
        ("linear", Ease::Linear),
        ("sine.inOut", Ease::SineInOut),
        ("circ", Ease::CircOut),
        ("hold", Ease::Hold),
        (" EASE-OUT ", Ease::EaseOut),
    ] {
        assert_eq!(Ease::from_name(name), Some((e, None)), "{name}");
    }
    let (e, note) = Ease::from_name("elastic.out(1, 0.3)").unwrap();
    assert_eq!(e, Ease::ElasticOut);
    assert_eq!(
        note.as_deref(),
        Some(
            "Klipolo reads elastic.out(1, 0.3) as elastic.out (amplitude 1, period 0.3); parametric GSAP eases use \
             their defaults."
        )
    );
    assert_eq!(
        Ease::from_name("back.out(2)").unwrap().1.as_deref(),
        Some("Klipolo reads back.out(2) as back.out (s = 1.70158); parametric GSAP eases use their defaults.")
    );
    // Only GSAP's parametric eases take settings.
    assert!(Ease::from_name("power2.out(3)").is_none());
    assert!(Ease::from_name("back.out(two)").is_none());
}

#[test]
fn every_ease_writes_a_name_that_reads_back_as_itself() {
    use Ease as E;
    for e in [
        E::Linear,
        E::Ease,
        E::EaseIn,
        E::EaseOut,
        E::EaseInOut,
        E::Power1In,
        E::Power1Out,
        E::Power1InOut,
        E::Power2In,
        E::Power2Out,
        E::Power2InOut,
        E::Power3In,
        E::Power3Out,
        E::Power3InOut,
        E::Power4In,
        E::Power4Out,
        E::Power4InOut,
        E::BackIn,
        E::BackOut,
        E::BackInOut,
        E::ElasticIn,
        E::ElasticOut,
        E::ElasticInOut,
        E::BounceIn,
        E::BounceOut,
        E::BounceInOut,
        E::SineIn,
        E::SineOut,
        E::SineInOut,
        E::ExpoIn,
        E::ExpoOut,
        E::ExpoInOut,
        E::CircIn,
        E::CircOut,
        E::CircInOut,
        E::Hold,
        E::Steps { n: 64 },
        E::CubicBezier {
            x1: 1e-7,
            y1: -1.25,
            x2: 0.123_456_789_012_345_67,
            y2: 2.5,
        },
        E::Spring {
            stiffness: 333.3,
            damping: 2.0 / 3.0,
            mass: 0.1,
        },
    ] {
        let name = e.name();
        assert_eq!(Ease::from_name(&name), Some((e, None)), "{name}");
    }
    assert_eq!(Ease::Linear.name(), "none");
    assert_eq!(Ease::Power3InOut.name(), "power3.inOut");
    assert_eq!(
        Ease::Spring {
            stiffness: 120.0,
            damping: 14.0,
            mass: 1.0
        }
        .name(),
        "spring(120, 14, 1)"
    );
}
