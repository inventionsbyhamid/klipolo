//! Scene text helpers: a scene drawn frame after frame with its notes kept, the white ink of a
//! frame, and the lower third's title box.

use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::timeline::ItemContent;

use super::{project_with_scene, scene_frame, scene_golden, scene_item};

/// The frame size the scene text harness draws at.
pub const H_SIZE: (u32, u32) = (640, 360);

/// One scene filling a frame of `H_SIZE` for its whole duration, drawn by one GPU backend frame after frame, with every
/// frame's notes kept.
pub struct SceneHarness {
    h: klipolo_engine::headless::Headless,
    p: Project,
    duration: f64,
    notes: Vec<String>,
}

/// A harness for the scene `json`.
pub async fn scene_headless(json: &str) -> SceneHarness {
    let duration = serde_json::from_str::<serde_json::Value>(json).unwrap()["duration"]
        .as_f64()
        .unwrap();
    let (p, registry) = project_with_scene("s", json, 0.0, duration);
    SceneHarness {
        h: klipolo_engine::headless::Headless::new(registry)
            .await
            .unwrap(),
        p,
        duration,
        notes: Vec::new(),
    }
}

impl SceneHarness {
    /// The frame at `t` seconds into the scene.
    pub async fn render_at(&mut self, t: f64) -> image::RgbaImage {
        let p = Arc::new(self.p.clone());
        let (img, notes) = scene_frame(&mut self.h, &p, t, H_SIZE).await;
        self.notes.extend(notes);
        img
    }

    /// The text setting `key` set to `text`, then the frame halfway through the scene (a lower third's hold).
    pub async fn render_with_param(&mut self, key: &str, text: &str) -> image::RgbaImage {
        if let ItemContent::Generator { params, .. } = &mut scene_item(&mut self.p, "s").content {
            params.insert(
                key.into(),
                klipolo_model::params::ParamValue::Text(text.into()),
            );
        }
        self.render_at(self.duration / 2.0).await
    }

    /// Every note the frames drawn so far said.
    pub fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }

    /// What the scene drawer did (its text's bytes among it).
    pub fn stats(&self) -> klipolo_engine::render::scene::SceneStats {
        self.h.scene_stats()
    }
}

/// A box in a frame's pixels: left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Bounds {
    /// Inside `outer`, a pixel of antialiasing allowed.
    pub fn within(&self, outer: Bounds) -> bool {
        self.x0 >= outer.x0 - 1.0
            && self.y0 >= outer.y0 - 1.0
            && self.x1 <= outer.x1 + 1.0
            && self.y1 <= outer.y1 + 1.0
    }

    pub fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
}

/// The lower third's title box in a frame of `H_SIZE`: its card's box in the frame, then the title's in the card.
pub fn lower_third_title_box_px() -> Bounds {
    let (w, h) = (f64::from(H_SIZE.0), f64::from(H_SIZE.1));
    let card = [0.04 * w, 0.62 * h, 0.6 * w, 0.26 * h];
    let [x, y, bw, bh] = [0.05, 0.08, 0.9, 0.4];
    Bounds {
        x0: card[0] + x * card[2],
        y0: card[1] + y * card[3],
        x1: card[0] + (x + bw) * card[2],
        y1: card[1] + (y + bh) * card[3],
    }
}

/// A pixel of white ink: bright and grey (the lower third's title; its role is blue-grey, its tab and rule orange).
fn white(p: &image::Rgba<u8>) -> bool {
    let [r, g, b, _] = p.0;
    let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
    lo >= 150 && hi - lo <= 40
}

/// The box holding the frame's white ink within `area`, if any.
pub fn ink_bounds_in(img: &image::RgbaImage, area: Bounds) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    for (x, y, p) in img.enumerate_pixels() {
        let (fx, fy) = (f64::from(x), f64::from(y));
        if fx < area.x0.floor()
            || fx > area.x1.ceil()
            || fy < area.y0.floor()
            || fy > area.y1.ceil()
            || !white(p)
        {
            continue;
        }
        let px = Bounds {
            x0: fx,
            y0: fy,
            x1: fx + 1.0,
            y1: fy + 1.0,
        };
        b = Some(match b {
            None => px,
            Some(o) => Bounds {
                x0: o.x0.min(px.x0),
                y0: o.y0.min(px.y0),
                x1: o.x1.max(px.x1),
                y1: o.y1.max(px.y1),
            },
        });
    }
    b
}

/// The box holding the frame's white ink (empty, outside everything, when there is none).
pub fn ink_bounds(img: &image::RgbaImage) -> Bounds {
    let all = Bounds {
        x0: 0.0,
        y0: 0.0,
        x1: f64::from(img.width()),
        y1: f64::from(img.height()),
    };
    ink_bounds_in(img, all).unwrap_or(Bounds {
        x0: f64::MAX,
        y0: f64::MAX,
        x1: f64::MIN,
        y1: f64::MIN,
    })
}

/// How tall the frame's white ink is.
pub fn ink_height(img: &image::RgbaImage) -> f64 {
    ink_bounds(img).height()
}

/// `img` against the golden `path` (`scene/<name>.png` under `tests/golden`): see `scene_golden`.
pub fn compare_golden(img: &image::RgbaImage, path: &str) {
    let name = path.trim_start_matches("scene/").trim_end_matches(".png");
    scene_golden(name, img);
}
