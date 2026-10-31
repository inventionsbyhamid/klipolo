//! Stickers and title cards made by an `ImageMaker` (a fake here) into the project's media folder: where they go, what
//! the asset says, and that a failure says why in one sentence and adds nothing.

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::{ImageFormat, Rgb, RgbImage};
use klipolo_analysis::providers::BoxFuture;
use klipolo_analysis::providers::images::{
    ImageError, ImageErrorKind, ImageMaker, ImageSize, MadeImage,
};
use klipolo_app::media::generated::{
    MakeError, make_sticker, make_title_card, sticker_prompt, title_card_prompt,
};
use klipolo_app::media::media_folder::slug;
use klipolo_app::media::sticker::ERASES_MOST_NOTE;
use klipolo_model::project::{AssetKind, FormatKind};

/// A stand-in for Muse Image: answers in turn, and remembers what it was asked.
struct Fake {
    answers: Mutex<VecDeque<Result<Vec<MadeImage>, ImageError>>>,
    asked: Mutex<Vec<(String, ImageSize, u8)>>,
}

impl Fake {
    fn answering(answers: Vec<Result<Vec<MadeImage>, ImageError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn drawing(png: Vec<u8>) -> Self {
        Self::answering(vec![Ok(vec![picture(png)])])
    }

    fn asked(&self) -> Vec<(String, ImageSize, u8)> {
        self.asked.lock().unwrap().clone()
    }
}

impl ImageMaker for Fake {
    fn made_with(&self) -> &str {
        "Muse Image"
    }

    fn make<'a>(
        &'a self,
        prompt: &'a str,
        size: ImageSize,
        n: u8,
    ) -> BoxFuture<'a, Result<Vec<MadeImage>, ImageError>> {
        self.asked
            .lock()
            .unwrap()
            .push((prompt.to_owned(), size, n));
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("an answer for each request");
        Box::pin(async move { answer })
    }
}

fn picture(png: Vec<u8>) -> MadeImage {
    MadeImage {
        png,
        revised_prompt: Some("A single paper plane sticker, bold outline".into()),
    }
}

fn paper_plane() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paper-plane.png"))
        .unwrap()
}

fn png_of(image: &RgbImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A project file in a fresh library folder (the file itself need not exist).
fn project(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("Trip.klipolo")
}

fn made_folder(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("Trip Media").join("Muse Image")
}

/// Every file in `folder`, by name; none when it doesn't exist.
fn files(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn failure(kind: ImageErrorKind, message: &str) -> ImageError {
    ImageError::new(kind, message, format!("{kind:?} for the log"))
}

#[tokio::test]
async fn a_sticker_is_cut_out_and_saved_in_the_projects_media_folder() {
    let dir = tempfile::tempdir().unwrap();
    let maker = Fake::drawing(paper_plane());

    let made = make_sticker(&maker, &project(&dir), "paper plane")
        .await
        .unwrap();

    let file = made_folder(&dir).join("paper-plane-1.png");
    assert_eq!(
        made.asset.path,
        file.canonicalize().unwrap().to_string_lossy()
    );
    assert!(Path::new(&made.asset.path).is_absolute());
    assert_eq!(made.asset.kind, AssetKind::Image);
    assert_eq!((made.asset.width, made.asset.height), (1600, 1600));
    assert_eq!(made.asset.name, "Paper plane sticker");
    assert_eq!(made.asset.duration_us, 5_000_000);
    assert!(!made.asset.has_audio);
    assert_eq!(made.made_with, "Muse Image");
    assert_eq!(made.note, None);
    assert_eq!(
        made.revised_prompt.as_deref(),
        Some("A single paper plane sticker, bold outline")
    );
    // Cut out: the corner is see-through.
    let saved = image::open(&file).unwrap().to_rgba8();
    assert_eq!(saved.get_pixel(5, 5)[3], 0);
    assert_eq!(saved.get_pixel(900, 800)[3], 255);
    // Asked once, for one square picture, in the sticker's words.
    assert_eq!(
        maker.asked(),
        [(
            "A single paper plane as a sticker illustration with a bold dark outline, centred with empty margin on all \
             sides, on a plain white background, nothing else"
                .to_owned(),
            ImageSize::Square,
            1
        )]
    );
    assert_eq!(
        sticker_prompt("cat"),
        maker.asked()[0].0.replace("paper plane", "cat")
    );
    // Nothing half-written is left beside it.
    assert_eq!(files(&made_folder(&dir)), ["paper-plane-1.png"]);
}

#[tokio::test]
async fn a_file_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(made_folder(&dir)).unwrap();
    std::fs::write(
        made_folder(&dir).join("paper-plane-1.png"),
        b"the person's own",
    )
    .unwrap();
    let maker = Fake::answering(vec![
        Ok(vec![picture(paper_plane())]),
        Ok(vec![picture(paper_plane())]),
    ]);

    let second = make_sticker(&maker, &project(&dir), "paper plane")
        .await
        .unwrap();
    let third = make_sticker(&maker, &project(&dir), "Paper plane")
        .await
        .unwrap();

    assert!(
        second.asset.path.ends_with("paper-plane-2.png"),
        "{}",
        second.asset.path
    );
    assert!(
        third.asset.path.ends_with("paper-plane-3.png"),
        "{}",
        third.asset.path
    );
    assert_ne!(second.asset.id, third.asset.id);
    assert_eq!(
        std::fs::read(made_folder(&dir).join("paper-plane-1.png")).unwrap(),
        b"the person's own"
    );
    assert_eq!(
        files(&made_folder(&dir)),
        [
            "paper-plane-1.png",
            "paper-plane-2.png",
            "paper-plane-3.png"
        ]
    );
}

/// A cut-out that would erase most of the picture keeps it whole, with the note.
#[tokio::test]
async fn a_sticker_the_cut_out_would_erase_is_kept_whole_with_a_note() {
    let dir = tempfile::tempdir().unwrap();
    // A white cup in a hairline that runs off the left edge: the flood would take its inside too.
    let cup = RgbImage::from_fn(300, 300, |x, y| {
        let line = (x <= 259 && (y == 40 || y == 259)) || (x == 259 && (40..=259).contains(&y));
        if line {
            Rgb([0, 0, 0])
        } else {
            Rgb([255, 255, 255])
        }
    });
    let maker = Fake::drawing(png_of(&cup));

    let made = make_sticker(&maker, &project(&dir), "white cup")
        .await
        .unwrap();

    assert_eq!(made.note.as_deref(), Some(ERASES_MOST_NOTE));
    let saved = image::open(&made.asset.path).unwrap().to_rgba8();
    assert!(saved.pixels().all(|p| p[3] == 255), "kept opaque");
    assert_eq!(saved.get_pixel(5, 5).0, [255, 255, 255, 255]);
}

/// 402, 429, 503, a timeout and a refused prompt each come back as the maker's one sentence, and nothing
/// is saved.
#[tokio::test]
async fn every_failure_says_why_and_adds_nothing() {
    let cases = [
        (
            ImageErrorKind::OutOfCredit,
            "Klipolo's credit for Muse Image has run out, so add your own Meta key in Settings to keep going.",
        ),
        (
            ImageErrorKind::Busy,
            "Muse Image is busy right now, so try again in a moment.",
        ),
        (
            ImageErrorKind::Refused,
            "Muse Image won't draw that, so try describing it another way.",
        ),
        (
            ImageErrorKind::Timeout,
            "Muse Image took too long to answer, so try again.",
        ),
        (
            ImageErrorKind::Other,
            "Muse Image couldn't make that picture, so try again.",
        ),
    ];
    for (kind, sentence) in cases {
        let dir = tempfile::tempdir().unwrap();
        let maker = Fake::answering(vec![
            Err(failure(kind, sentence)),
            Err(failure(kind, sentence)),
        ]);

        let sticker = make_sticker(&maker, &project(&dir), "paper plane")
            .await
            .unwrap_err();
        let card = make_title_card(&maker, &project(&dir), "Day 1", FormatKind::Portrait)
            .await
            .unwrap_err();

        for error in [sticker, card] {
            assert_eq!(error.user_message(), sentence);
            assert!(
                matches!(&error, MakeError::Maker(e) if e.kind == kind),
                "{error:?}"
            );
        }
        assert!(
            !dir.path().join("Trip Media").exists(),
            "{kind:?} left a folder"
        );
    }
}

#[tokio::test]
async fn a_picture_that_cannot_be_read_says_so_and_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let broken = || Ok(vec![picture(b"\x89PNG\r\n\x1a\n cut short".to_vec())]);
    let maker = Fake::answering(vec![broken(), broken()]);

    let sticker = make_sticker(&maker, &project(&dir), "paper plane")
        .await
        .unwrap_err();
    let card = make_title_card(&maker, &project(&dir), "Day 1", FormatKind::Square)
        .await
        .unwrap_err();

    for error in [sticker, card] {
        assert_eq!(
            error.user_message(),
            "Muse Image sent a picture Klipolo couldn't read, so try again."
        );
    }
    assert_eq!(files(&made_folder(&dir)), Vec::<String>::new());
}

#[tokio::test]
async fn a_picture_over_4096_pixels_a_side_is_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let huge = png_of(&RgbImage::from_pixel(4097, 4, Rgb([20, 30, 90])));
    let maker = Fake::drawing(huge);

    let error = make_title_card(&maker, &project(&dir), "Day 1", FormatKind::Landscape)
        .await
        .unwrap_err();

    assert_eq!(
        error.user_message(),
        "Muse Image sent a picture Klipolo couldn't read, so try again."
    );
    assert_eq!(files(&made_folder(&dir)), Vec::<String>::new());
}

#[tokio::test]
async fn an_empty_prompt_asks_for_words_without_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let maker = Fake::answering(vec![]);

    let error = make_sticker(&maker, &project(&dir), "  ")
        .await
        .unwrap_err();

    assert_eq!(error.user_message(), "Say what the picture should show.");
    assert!(maker.asked().is_empty());
}

#[tokio::test]
async fn try_again_after_a_failure_works() {
    let dir = tempfile::tempdir().unwrap();
    let busy = "Muse Image is busy right now, so try again in a moment.";
    let maker = Fake::answering(vec![
        Err(failure(ImageErrorKind::Busy, busy)),
        Ok(vec![picture(paper_plane())]),
    ]);

    let first = make_sticker(&maker, &project(&dir), "paper plane").await;
    let again = make_sticker(&maker, &project(&dir), "paper plane").await;

    assert_eq!(first.unwrap_err().user_message(), busy);
    assert!(again.unwrap().asset.path.ends_with("paper-plane-1.png"));
}

/// Muse Image draws only the background: its own lettering can't be held to the title's words, spelling, type or copy
/// rules, so the prompt asks for none, and Klipolo's title-card template draws the exact title on top.
#[test]
fn a_title_card_asks_for_a_background_with_no_lettering() {
    assert_eq!(
        title_card_prompt("Day 1 in Lisbon"),
        "A background for a title card about Day 1 in Lisbon, with no text, letters, numbers or signs anywhere, and \
         calm, uncluttered space in the middle third for a title"
    );
}

#[tokio::test]
async fn a_title_card_stays_opaque_in_the_formats_shape() {
    let shapes = [
        (FormatKind::Landscape, ImageSize::Landscape, (175, 100)),
        (FormatKind::Portrait, ImageSize::Portrait, (100, 175)),
        (FormatKind::Square, ImageSize::Square, (120, 120)),
    ];
    for (format, size, (w, h)) in shapes {
        let dir = tempfile::tempdir().unwrap();
        let card = png_of(&RgbImage::from_pixel(w, h, Rgb([20, 30, 90])));
        let maker = Fake::drawing(card.clone());

        let made = make_title_card(&maker, &project(&dir), "Day 1 in Lisbon", format)
            .await
            .unwrap();

        assert_eq!(
            maker.asked(),
            [(title_card_prompt("Day 1 in Lisbon"), size, 1)]
        );
        assert!(
            made.asset.path.ends_with("title-day-1-in-lisbon-1.png"),
            "{}",
            made.asset.path
        );
        assert_eq!(
            std::fs::read(&made.asset.path).unwrap(),
            card,
            "kept as drawn"
        );
        assert_eq!((made.asset.width, made.asset.height), (w, h));
        assert_eq!(made.asset.name, "Day 1 in Lisbon title card background");
        assert_eq!(made.made_with, "Muse Image");
        assert_eq!(made.note, None);
    }
}

#[test]
fn slugs_are_short_and_stay_in_the_folder() {
    assert_eq!(slug("Paper plane!", "sticker"), "paper-plane");
    assert_eq!(
        slug("A big red coffee cup with steam", "sticker"),
        "a-big-red-coffee"
    );
    assert_eq!(slug("../../etc/passwd", "sticker"), "etc-passwd");
    assert_eq!(slug("  ", "sticker"), "sticker");
    assert_eq!(slug("☕", "sticker"), "sticker");
    assert!(slug(&"x".repeat(200), "sticker").len() <= 32);
}
