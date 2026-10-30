//! A development tool for the Muse Image smoke test, never part of the app: it draws stickers and title cards for real
//! and saves them as the app would, or cuts out a PNG you already have, with the time it took.
//!
//! ```text
//! klipolo-image sticker <prompt> --out <folder> [--fast]
//! klipolo-image title <landscape|portrait|square> <prompt> --out <folder> [--fast]
//! klipolo-image cutout <in.png> <out.png>
//! ```
//!
//! `sticker` and `title` call Muse Image for real, about $0.01 a picture. `MODEL_API_KEY` (a Meta key) or
//! `KLIPOLO_RELAY_TOKEN` (a relay device token) comes from the environment and is never printed; with both, the key is
//! used. `KLIPOLO_SPEND_LEDGER` (a CSV file), when set, gets one row per call; `KLIPOLO_META_BASE` and
//! `KLIPOLO_RELAY_BASE` point the calls elsewhere. Pictures go where the app would put them for a project
//! `<folder>/Smoke.klipolo`, that is `<folder>/Smoke Media/Muse Image/`, and Muse Image's own picture is kept as
//! `<folder>/drawn-<time>-1.png` to compare with the cut-out. `--fast` asks for one generation pass
//! (`reasoning_strength: low`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use klipolo_analysis::providers::BoxFuture;
use klipolo_analysis::providers::images::{ImageError, ImageMaker, ImageSize, MadeImage};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::meta_images::MetaImages;
use klipolo_app::media::generated::{MadeAsset, MakeError, make_sticker, make_title_card};
use klipolo_app::media::sticker::cut_out_white;
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::project::FormatKind;

const KEY_ENV: &str = "MODEL_API_KEY";
const TOKEN_ENV: &str = "KLIPOLO_RELAY_TOKEN";
const USAGE: &str = "usage:
  klipolo-image sticker <prompt> --out <folder> [--fast]
  klipolo-image title <landscape|portrait|square> <prompt> --out <folder> [--fast]
  klipolo-image cutout <in.png> <out.png>";

#[tokio::main]
async fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    match command.as_str() {
        "cutout" => match rest {
            [input, output] => cutout(Path::new(input), Path::new(output)),
            _ => Err(USAGE.into()),
        },
        "sticker" => {
            let ask = Ask::parse(rest, 1)?;
            let (maker, used) = maker(ask.fast)?;
            let keep = KeepDrawn(&maker, ask.out.clone());
            println!(
                "Asking Muse Image with a {used} for a sticker of {:?}",
                ask.words[0]
            );
            let started = Instant::now();
            let made = make_sticker(&keep, &ask.project(), &ask.words[0]).await;
            report(made, started)
        }
        "title" => {
            let ask = Ask::parse(rest, 2)?;
            let format = match ask.words[0].as_str() {
                "landscape" | "16:9" => FormatKind::Landscape,
                "portrait" | "9:16" => FormatKind::Portrait,
                "square" | "1:1" => FormatKind::Square,
                _ => return Err(USAGE.into()),
            };
            let (maker, used) = maker(ask.fast)?;
            println!(
                "Asking Muse Image with a {used} for a {} title card of {:?}",
                ask.words[0], ask.words[1]
            );
            let started = Instant::now();
            let made = make_title_card(&maker, &ask.project(), &ask.words[1], format).await;
            report(made, started)
        }
        _ => Err(USAGE.into()),
    }
}

/// A `sticker` or `title` command's words, folder and speed.
struct Ask {
    words: Vec<String>,
    out: PathBuf,
    fast: bool,
}

impl Ask {
    fn parse(args: &[String], words: usize) -> Result<Self, String> {
        let mut ask = Ask {
            words: Vec::new(),
            out: PathBuf::new(),
            fast: false,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--out" => ask.out = args.next().map(PathBuf::from).ok_or(USAGE)?,
                "--fast" => ask.fast = true,
                _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
                _ => ask.words.push(arg.clone()),
            }
        }
        if ask.words.len() != words || ask.out.as_os_str().is_empty() {
            return Err(USAGE.into());
        }
        std::fs::create_dir_all(&ask.out)
            .map_err(|e| format!("Could not make {}: {e}", ask.out.display()))?;
        Ok(ask)
    }

    /// The project the pictures are made for (the file itself isn't needed).
    fn project(&self) -> PathBuf {
        self.out.join("Smoke.klipolo")
    }
}

/// Muse Image with the credentials the environment gives, and which kind it used (never its value).
fn maker(fast: bool) -> Result<(MetaImages, &'static str), String> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (client, used) = match (var(KEY_ENV), var(TOKEN_ENV)) {
        (Some(key), _) => (MetaClient::direct(SecretString::from(key)), "Meta key"),
        (None, Some(token)) => (MetaClient::relay(SecretString::from(token)), "relay token"),
        (None, None) => return Err(format!("Set {KEY_ENV} or {TOKEN_ENV}.")),
    };
    let images = MetaImages::new(Meta::new(client));
    Ok((if fast { images.fast() } else { images }, used))
}

fn report(made: Result<MadeAsset, MakeError>, started: Instant) -> Result<(), String> {
    let seconds = started.elapsed().as_secs_f64();
    match made {
        Ok(made) => {
            println!("Made in {seconds:.1} s: {}", made.asset.path);
            println!(
                "  {} ({}x{}), made with {}",
                made.asset.name, made.asset.width, made.asset.height, made.made_with
            );
            if let Some(note) = made.note {
                println!("  note: {note}");
            }
            if let Some(revised) = made.revised_prompt {
                println!("  revised prompt: {revised}");
            }
            Ok(())
        }
        Err(error) => Err(format!(
            "Failed after {seconds:.1} s: {}\n  detail: {error}",
            error.user_message()
        )),
    }
}

fn cutout(input: &Path, output: &Path) -> Result<(), String> {
    let png = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let started = Instant::now();
    let keyed = cut_out_white(&png);
    let took = started.elapsed();
    match keyed {
        Ok(keyed) => {
            std::fs::write(output, &keyed.png).map_err(|e| format!("{}: {e}", output.display()))?;
            println!(
                "Cut out in {} ms, kept {:.1}%: {}",
                took.as_millis(),
                keyed.kept_fraction * 100.0,
                output.display()
            );
            Ok(())
        }
        Err(error) => Err(format!(
            "No cut-out ({} ms): {error}{}",
            took.as_millis(),
            error
                .note()
                .map(|n| format!("\n  note: {n}"))
                .unwrap_or_default()
        )),
    }
}

/// Muse Image, keeping each picture as drawn beside the folder, to compare with its cut-out.
struct KeepDrawn<'a>(&'a MetaImages, PathBuf);

impl ImageMaker for KeepDrawn<'_> {
    fn made_with(&self) -> &str {
        self.0.made_with()
    }

    fn make<'a>(
        &'a self,
        prompt: &'a str,
        size: ImageSize,
        n: u8,
    ) -> BoxFuture<'a, Result<Vec<MadeImage>, ImageError>> {
        Box::pin(async move {
            let images = self.0.make(prompt, size, n).await?;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            for (i, image) in images.iter().enumerate() {
                let file = self.1.join(format!("drawn-{stamp}-{}.png", i + 1));
                match std::fs::write(&file, &image.png) {
                    Ok(()) => println!("  drawn: {}", file.display()),
                    Err(e) => eprintln!("  could not keep the drawn picture: {e}"),
                }
            }
            Ok(images)
        })
    }
}
