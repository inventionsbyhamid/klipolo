//! Captions as data: the styles (`style`, `styles`), the checks of a project's caption
//! settings, the person's look for caption words (`looks`), and the pages made from the shown words (`said`, `pages`:
//! keywords and casing included), their lines (`lines`, shared by the drawn captions and the caption files), and
//! those caption files (`sidecar`: SRT and WebVTT).

mod casing;
pub mod keywords;
pub mod lines;
pub mod looks;
mod motion;
mod page_builder;
mod page_close;
pub mod pages;
mod paging;
mod paint;
mod said;
mod settings;
pub mod sidecar;
mod sidecar_write;
pub mod style;
mod style_check;
pub mod styles;

pub use casing::shown;
pub use keywords::Keyword;
pub use pages::{captions, captions_in, page_at, previewed, Captions, Page, PageWord, Pages};
pub use said::{word_source, BREAK_GAP_US};
pub(crate) use settings::validate_settings;
