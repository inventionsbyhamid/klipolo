//! Chat's `followSubject` as it runs once the plan is made: in a clip the card follows already, who chat keeps ("the
//! person on the left") is one of the people that follow found, made from what it tracked at no charge, whatever the
//! model calls them; and the reply says what came of the follow, never that a follow that found nobody is done. Fakes
//! for SAM, Muse Spark, the small copy and the language model: nothing here reaches Meta.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::VideoError;
use klipolo_analysis::providers::images::ImageMaker;
use klipolo_analysis::providers::{
    BoxFuture, Heard, ProviderError, ScreenBox, SubjectTracker, TrackedBox, TrackedFrame, Tracking,
    VoiceAnswer, VoiceMatcher,
};
use klipolo_analysis::subject::voices::VoiceTurn;
use klipolo_analysis::subject::{PERSON, SubjectCopy, frames_for};
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::follow::{NO_ONE_THERE, Place, names_a_person, phrase_of, place_of};
use klipolo_app::chat::{
    ChatOutcome, ChatRequest, ChatSession, Findings, MemoryProjects, Models, Projects, Readiness,
};
use klipolo_app::speaker::{Speaker, Trackers, Voices};
use klipolo_chat::{
    AnalysisState, FollowFocus, FunctionCall, LanguageModel, ModelError, ModelRequest,
    ModelResponse,
};
use klipolo_model::apply_ops;
use klipolo_model::fill_focus::{FillFocus, Keep, Voice, Who};
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::speech::Levels;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use serde_json::Value;

fn s(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

/// SAM as it answered on the demo's footage: two people far apart on every frame for "person" (7 on the left, 3 on
/// the right), and nobody for any other phrase ("woman astronaut"). Keeps every phrase it was asked for. With
/// `at_edge`, the one on the left stands at the picture's edge.
#[derive(Default)]
struct TwoPeople {
    asked: Mutex<Vec<String>>,
    at_edge: bool,
}

impl TwoPeople {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl SubjectTracker for TwoPeople {
    fn model(&self) -> &str {
        "fake-sam"
    }

    fn max_frames(&self) -> u64 {
        15_000
    }

    fn estimate_usd(&self, frames: u64) -> f64 {
        frames.max(50) as f64 * 0.0002
    }

    fn track<'a>(
        &'a self,
        video: &'a Path,
        phrase: &'a str,
    ) -> BoxFuture<'a, Result<Tracking, ProviderError>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(phrase.to_owned());
            let frames: u32 = std::fs::read_to_string(video).unwrap().parse().unwrap();
            let at = |id, x: f64| TrackedBox {
                id,
                left: x - 0.1,
                top: 0.1,
                right: x + 0.1,
                bottom: 1.0,
                confidence: Some(0.9),
            };
            let left = if self.at_edge { 0.1 } else { 0.25 };
            let people = if phrase == PERSON {
                vec![at(3, 0.75), at(7, left)]
            } else {
                Vec::new()
            };
            Ok(Tracking {
                frames: (0..frames)
                    .filter(|_| !people.is_empty())
                    .map(|frame| TrackedFrame {
                        frame,
                        boxes: people.clone(),
                    })
                    .collect(),
                frames_billed: u64::from(frames),
                cost_usd: f64::from(frames.max(50)) * 0.0002,
                cut_off: false,
            })
        })
    }
}

/// Says the person on the left speaks, except in a window starting from 80 s to 100 s (the one on the right).
#[derive(Default)]
struct FakeSpark {
    asked: AtomicUsize,
}

impl VoiceMatcher for FakeSpark {
    fn model(&self) -> &str {
        "fake-spark"
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        0.001 * seconds
    }

    fn who_speaks<'a>(
        &'a self,
        video: &'a Path,
        _people: &'a [ScreenBox],
    ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let start: Us = std::fs::read_to_string(video).unwrap().parse().unwrap();
            let person = if (s(80.0)..s(100.0)).contains(&start) {
                2
            } else {
                1
            };
            Ok(VoiceAnswer {
                heard: Heard::Person { person },
                cost_usd: 0.003,
            })
        })
    }
}

struct FakeTrackers {
    sam: Arc<TwoPeople>,
    spark: Arc<FakeSpark>,
}

impl Trackers for FakeTrackers {
    fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>> {
        Box::pin(async move { Ok(self.sam.clone() as Arc<dyn SubjectTracker>) })
    }

    fn credit_left(&self) -> BoxFuture<'_, Option<f64>> {
        Box::pin(async { None })
    }

    fn matcher(&self) -> BoxFuture<'_, Option<Arc<dyn VoiceMatcher>>> {
        Box::pin(async move { Some(self.spark.clone() as Arc<dyn VoiceMatcher>) })
    }
}

/// The copy holds its frame count; the copy with sound, where it starts.
struct FakeCopy;

impl SubjectCopy for FakeCopy {
    fn copy(
        &self,
        _src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        std::fs::write(out, frames_for(range.end - range.start, fps).to_string()).unwrap();
        Ok(vec![])
    }

    fn copy_with_sound(
        &self,
        _src: &Path,
        range: RangeUs,
        _fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<(), VideoError> {
        std::fs::write(out, range.start.to_string()).unwrap();
        Ok(())
    }
}

/// The talk's transcript: A to 80 s and from 100 s, C in between, in its sound time.
struct Heard2;

impl Voices for Heard2 {
    fn turns(&self, _asset_id: &str) -> Vec<VoiceTurn> {
        let turn = |from: f64, to: f64, label: &str| VoiceTurn {
            start_us: s(from),
            end_us: s(to),
            voice: Voice {
                part: 0,
                label: label.into(),
            },
        };
        vec![
            turn(60.0, 80.0, "A"),
            turn(80.0, 100.0, "C"),
            turn(100.0, 120.0, "A"),
        ]
    }
}

/// The analysis is done; nothing chat needs here comes from it.
struct Found;

impl Findings for Found {
    fn found(&self, _asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        Some(Arc::new(AssetAnalysis::default()))
    }

    fn state(&self, _asset_id: &str) -> AnalysisState {
        AnalysisState::Done
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        HashMap::new()
    }
}

/// A model that answers every request in turn from its script: its calls, then its closing words once it is told
/// what came of them. Past the script, it is done: no words, no call.
struct Steps {
    answers: Mutex<VecDeque<ModelResponse>>,
}

impl Steps {
    fn new(answers: Vec<ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
        })
    }
}

impl LanguageModel for Steps {
    fn respond<'a>(
        &'a self,
        _request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            Ok(self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ModelResponse {
                    text: String::new(),
                    calls: Vec::new(),
                    usage_usd: 0.0,
                }))
        })
    }
}

struct Paying(Arc<Steps>);

impl Models for Paying {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        let model = self.0.clone() as Arc<dyn LanguageModel>;
        Box::pin(async move { Readiness::Ready { model } })
    }

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        Box::pin(async { Err("No pictures here.".to_owned()) })
    }

    fn picture_usd(&self) -> f64 {
        0.0
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

/// A two-person talk (16:9, a minute of it: 60 to 120 s, at 0 to 60 s on the timeline) in a vertical version, with
/// the speaker following it through the fakes.
struct Setup {
    dir: tempfile::TempDir,
    project: Project,
    sam: Arc<TwoPeople>,
    spark: Arc<FakeSpark>,
    speaker: Arc<Speaker>,
}

fn setup() -> Setup {
    setup_with(TwoPeople::default())
}

/// `setup` with SAM as given.
fn setup_with(sam: TwoPeople) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.mp4");
    std::fs::write(&file, b"the talk's bytes").unwrap();
    let mut project = Project::new("Talk");
    project.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: file.to_string_lossy().into_owned(),
        name: "talk.mp4".into(),
        duration_us: s(600.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    project.settings.formats = vec![Landscape, Portrait];
    project.settings.primary_format = Portrait;
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, s(60.0), s(120.0));
    clip.id = "c1".into();
    project.timeline.main = vec![clip];
    let (sam, spark) = (Arc::new(sam), Arc::new(FakeSpark::default()));
    let speaker = Arc::new(
        Speaker::new(
            Arc::new(FakeTrackers {
                sam: sam.clone(),
                spark: spark.clone(),
            }),
            Arc::new(FakeCopy),
            dir.path().join("analysis"),
            dir.path().join("work"),
            Arc::new(|_| {}),
        )
        .with_voices(Arc::new(Heard2)),
    );
    Setup {
        dir,
        project,
        sam,
        spark,
        speaker,
    }
}

/// The card's Speaker in 9:16, as the person clicked it: tracked and matched (paid), and landed.
async fn followed_by_the_card(setup: &mut Setup) {
    let ids = ["c1".to_owned()];
    let followed = (setup.speaker)
        .follow(&setup.project, &ids, &[Portrait], PERSON)
        .await
        .unwrap();
    assert!(followed.paid_usd > 0.0);
    setup.project = apply_ops(&setup.project, &followed.ops(&setup.project)).unwrap();
    let (keep, people) = kept(&setup.project);
    assert_eq!((keep, people), (Keep::Speaker, 2));
}

/// Who the clip's 9:16 crop keeps, and how many people it chooses between.
fn kept(p: &Project) -> (Keep, usize) {
    match p.timeline.main[0].fill_focus.get(&Portrait) {
        Some(FillFocus::Speaker { keep, people, .. }) => (keep.clone(), people.len()),
        other => panic!("not following: {other:?}"),
    }
}

/// A chat on `setup`'s project with its speaker, the model answering `calls` and then `closing`.
fn chat(
    setup: &Setup,
    calls: Vec<FunctionCall>,
    closing: &str,
) -> (Arc<ChatSession>, Arc<MemoryProjects>) {
    let model = Steps::new(vec![
        ModelResponse {
            text: String::new(),
            calls,
            usage_usd: 0.004,
        },
        ModelResponse {
            text: closing.into(),
            calls: Vec::new(),
            usage_usd: 0.002,
        },
    ]);
    let projects = Arc::new(
        MemoryProjects::open(setup.project.clone(), setup.dir.path().join("talk.klipolo")).unwrap(),
    );
    let session = Arc::new(ChatSession::new(
        projects.clone(),
        Arc::new(Found),
        Arc::new(Paying(model)),
    ));
    session.set_speaker(setup.speaker.clone());
    (session, projects)
}

fn fc(call_id: &str, name: &str, arguments: &str) -> FunctionCall {
    FunctionCall {
        name: name.into(),
        call_id: call_id.into(),
        arguments: arguments.into(),
    }
}

fn request(text: &str) -> ChatRequest {
    serde_json::from_value(serde_json::json!({
        "id": "m1",
        "text": text,
        "selection": null,
        "item": null,
        "playheadUs": 41_700_000,
        "format": "portrait",
        "baseRevision": 0,
    }))
    .unwrap()
}

fn reply(outcome: &ChatOutcome) -> Value {
    let v = serde_json::to_value(outcome).unwrap();
    assert_eq!(v["type"], "reply", "{v:#}");
    v
}

/// "Keep the person on the left in frame." in a clip the card follows: the model names her as the scene log does and
/// keeps P1 (People's leftmost) all along. The follow the card made is used again: nothing is tracked or matched,
/// nothing is paid, she is kept in frame as the card's Left keeps her, and the reply says so at no charge.
#[tokio::test(flavor = "multi_thread")]
async fn chat_keeps_the_person_on_the_left_from_the_cards_follow_at_no_charge() {
    let mut setup = setup();
    followed_by_the_card(&mut setup).await;
    let (tracked, matched, spent) = (
        setup.sam.asked(),
        setup.spark.asked.load(Ordering::SeqCst),
        setup.speaker.spent_usd(),
    );
    assert_eq!(tracked, [PERSON]);
    let (session, projects) = chat(
        &setup,
        vec![fc(
            "f",
            "followSubject",
            r#"{"description": "woman astronaut", "focus": [{"fromSec": 0, "toSec": 60, "person": "P1"}]}"#,
        )],
        "Done. The person on the left stays in frame for vertical and square.",
    );
    let r = reply(
        &session
            .send(request("Keep the person on the left in frame."))
            .await
            .unwrap(),
    );
    assert_eq!(setup.sam.asked(), tracked, "nothing tracked again");
    assert_eq!(
        setup.spark.asked.load(Ordering::SeqCst),
        matched,
        "nothing matched again"
    );
    assert_eq!(setup.speaker.spent_usd(), spent, "nothing paid");
    assert_eq!(
        r["text"],
        "Done. The person on the left stays in frame for vertical and square."
    );
    assert_eq!(
        r["notes"],
        serde_json::json!([
            "The vertical version keeps the person on the left in frame now. No charge: that footage was tracked before."
        ])
    );
    assert!(r["applied"].is_object(), "{r:#}");
    let now = projects.now().unwrap().project;
    assert_eq!(kept(&now), (Keep::Person { person: 1 }, 2));
}

/// A part of the clip chat names keeps P2 there and whoever speaks elsewhere: the card shows it as chat's own (Custom
/// by chat), made from the card's follow at no charge.
#[tokio::test(flavor = "multi_thread")]
async fn a_part_chat_names_keeps_its_person_there_from_the_cards_follow() {
    let mut setup = setup();
    followed_by_the_card(&mut setup).await;
    let tracked = setup.sam.asked();
    let (session, projects) = chat(
        &setup,
        vec![fc(
            "f",
            "followSubject",
            r#"{"description": "man astronaut", "focus": [{"fromSec": 30, "toSec": 60, "person": "P2"}]}"#,
        )],
        "The person on the right stays in frame from 0:30.",
    );
    let r = reply(
        &session
            .send(request("Keep the person on the right in frame from 0:30."))
            .await
            .unwrap(),
    );
    assert_eq!(setup.sam.asked(), tracked, "nothing tracked again");
    let notes = r["notes"].to_string();
    assert!(
        notes.contains("No charge: that footage was tracked before."),
        "{r:#}"
    );
    let now = projects.now().unwrap().project;
    let (keep, people) = kept(&now);
    assert_eq!(people, 2);
    let Keep::Ranges { ranges } = keep else {
        panic!("{keep:?}")
    };
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(
        (ranges[0].start_us, ranges[0].end_us, ranges[0].who),
        (s(90.0), s(120.0), Who::Person { person: 2 })
    );
}

/// A follow that finds nobody: the model's words, written once it was told the call was done, said it was; the reply
/// says what came of it instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_follow_that_finds_nobody_says_so_instead_of_done() {
    let setup = setup();
    let (session, projects) = chat(
        &setup,
        vec![fc(
            "f",
            "followSubject",
            r#"{"description": "red balloon", "focus": null}"#,
        )],
        "Done. The red balloon stays in frame.",
    );
    let r = reply(
        &session
            .send(request("Follow the red balloon."))
            .await
            .unwrap(),
    );
    assert_eq!(setup.sam.asked(), ["red balloon"]);
    // A minute at 2 frames a second: 120 frames.
    assert_eq!(
        r["text"],
        format!("{NO_ONE_THERE} Tracking cost $0.02."),
        "{r:#}"
    );
    assert_eq!(r["notes"], serde_json::json!([]), "{r:#}");
    assert_eq!(projects.revision(), 0, "nothing changed");
    assert!(session.turns()[0].reply.starts_with(NO_ONE_THERE));
}

/// The same beside an edit that lands: what landed is said plainly, and why the follow didn't beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_follow_that_finds_nobody_beside_an_edit_says_only_what_landed() {
    let setup = setup();
    let (session, _projects) = chat(
        &setup,
        vec![
            fc(
                "v",
                "setFormat",
                r#"{"format": "square", "keepOthers": true}"#,
            ),
            fc(
                "f",
                "followSubject",
                r#"{"description": "red balloon", "focus": null}"#,
            ),
        ],
        "Done. It is square too, and the red balloon stays in frame.",
    );
    let r = reply(
        &session
            .send(request("Make it square too and follow the red balloon."))
            .await
            .unwrap(),
    );
    assert_eq!(r["text"], "The format is changed.", "{r:#}");
    let notes = r["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{r:#}");
    assert!(
        notes[0].as_str().unwrap().starts_with(NO_ONE_THERE),
        "{r:#}"
    );
    assert!(r["applied"].is_object(), "{r:#}");
}

/// The one-message short's own follow ("followSubject on the speaker", beside its cut and its format): whatever the
/// model calls the person, the people come from the person the card tracked, at no charge, and who is kept is chosen
/// among them by place; the follow lands with the cut once it is applied.
#[tokio::test(flavor = "multi_thread")]
async fn the_shorts_follow_reuses_the_cards_tracking_whatever_it_calls_the_person() {
    for (description, keep, who) in [
        ("speaker", Keep::Speaker, "speaker"),
        ("the speaker", Keep::Speaker, "speaker"),
        ("person", Keep::Speaker, "speaker"),
        ("woman astronaut", Keep::Speaker, "speaker"),
        (
            "person on the left",
            Keep::Person { person: 1 },
            "person on the left",
        ),
        (
            "the one on the right",
            Keep::Person { person: 2 },
            "person on the right",
        ),
    ] {
        let mut setup = setup();
        followed_by_the_card(&mut setup).await;
        let (tracked, matched, spent) = (
            setup.sam.asked(),
            setup.spark.asked.load(Ordering::SeqCst),
            setup.speaker.spent_usd(),
        );
        let follow = format!(r#"{{"description": "{description}", "focus": null}}"#);
        let (session, projects) = chat(
            &setup,
            vec![
                fc(
                    "k",
                    "keepOnly",
                    r#"{"ranges": [{"startSec": 0, "endSec": 40}]}"#,
                ),
                fc(
                    "v",
                    "setFormat",
                    r#"{"format": "portrait", "keepOthers": true}"#,
                ),
                fc("f", "followSubject", &follow),
            ],
            "Here is your vertical short. Review the cut below.",
        );
        let r = reply(
            &session
                .send(request("Make a vertical short of 0:00 to 0:40."))
                .await
                .unwrap(),
        );
        assert_eq!(
            setup.sam.asked(),
            tracked,
            "{description}: nothing tracked again"
        );
        assert_eq!(
            setup.spark.asked.load(Ordering::SeqCst),
            matched,
            "{description}: nothing matched again"
        );
        assert_eq!(
            setup.speaker.spent_usd(),
            spent,
            "{description}: nothing paid"
        );
        assert_eq!(
            r["text"],
            "Here is your vertical short. Review the cut below."
        );
        assert_eq!(
            r["notes"],
            serde_json::json!([format!(
                "The vertical version keeps the {who} in frame once you apply this. No charge: that footage was tracked before."
            )]),
            "{description}: {r:#}"
        );
        let id = r["proposal"]["id"].as_str().expect("the cut").to_owned();
        session.apply(&id).unwrap();
        let now = projects.now().unwrap().project;
        let clip = &now.timeline.main[0];
        assert_eq!(
            (clip.source_in_us, clip.source_out_us),
            (s(60.0), s(100.0)),
            "{description}"
        );
        assert_eq!(kept(&now), (keep, 2), "{description}");
    }
}

/// What chat tracks for a follow: a person whenever its focus names who to keep (People's P1, P2 are the people a
/// follow of a person found) or its words name a person, whatever they call them; else the thing its words name. Who
/// its words place is read only for a person.
#[test]
fn a_follow_of_a_person_tracks_a_person_and_reads_their_place() {
    let p1 = [FollowFocus {
        range: RangeUs {
            start: 0,
            end: s(60.0),
        },
        who: Who::Person { person: 1 },
    }];
    for person in [
        "speaker",
        "the speaker",
        "person",
        "the person",
        "woman astronaut",
        "two astronauts",
        "person on the left",
        "the one on the right",
        "left",
        "both of them",
        "the host",
        "me",
    ] {
        assert!(names_a_person(person), "{person}");
        assert_eq!(phrase_of(person, &[]), PERSON, "{person}");
    }
    for thing in [
        "the dog",
        "red balloon",
        "license plate",
        "the car on the left",
        "all the cars",
    ] {
        assert!(!names_a_person(thing), "{thing}");
        assert_eq!(place_of(thing), None, "{thing}");
    }
    assert_eq!(phrase_of("the dog", &[]), "dog");
    assert_eq!(phrase_of("the dog", &p1), PERSON, "a focus names people");
    assert_eq!(phrase_of("woman astronaut", &p1), PERSON);
    assert_eq!(place_of("person on the left"), Some(Place::Left));
    assert_eq!(place_of("the rightmost speaker"), Some(Place::Right));
    assert_eq!(place_of("the woman in the middle"), Some(Place::Middle));
    assert_eq!(place_of("both of them"), Some(Place::Everyone));
    assert_eq!(place_of("woman astronaut"), None);
}

/// A sticker with no place given and a follow of the speaker, in one message: once the follow's path lands with it, the
/// sticker goes beside the speaker's head where the path keeps them. The speaker stands near the picture's left edge,
/// where the 9:16 crop stops, so their head there is left of the middle and the sticker keeps off it.
#[tokio::test(flavor = "multi_thread")]
async fn a_sticker_moves_beside_the_head_where_the_follow_keeps_the_speaker() {
    let setup = setup_with(TwoPeople {
        at_edge: true,
        ..TwoPeople::default()
    });
    let heart = klipolo_chat::LibraryItem {
        name: "Heart".into(),
        kind: klipolo_chat::MediaKind::Image,
        asset: Asset {
            id: "klipolo-pack-heart".into(),
            kind: AssetKind::Image,
            path: "/pack/heart.png".into(),
            name: "heart.png".into(),
            duration_us: 0,
            width: 1024,
            height: 1024,
            fps: 0.0,
            rotation: None,
            has_audio: false,
            audio_lead_us: None,
            made_with: None,
        },
        pack: true,
        tags: vec!["heart".into()],
    };
    let model = Steps::new(vec![
        ModelResponse {
            text: String::new(),
            calls: vec![
                fc(
                    "f",
                    "followSubject",
                    r#"{"description": "speaker", "focus": null}"#,
                ),
                fc(
                    "h",
                    "addImage",
                    r#"{"file": "Heart", "size": null, "position": null, "atSec": 10, "atWords": null, "durationSec": null}"#,
                ),
            ],
            usage_usd: 0.004,
        },
        ModelResponse {
            text: "The speaker stays in frame, with a heart beside them at 0:10.".into(),
            calls: Vec::new(),
            usage_usd: 0.002,
        },
    ]);
    let projects = Arc::new(
        MemoryProjects::open(setup.project.clone(), setup.dir.path().join("talk.klipolo"))
            .unwrap()
            .with_pack(vec![heart]),
    );
    let session = Arc::new(ChatSession::new(
        projects.clone(),
        Arc::new(Found),
        Arc::new(Paying(model)),
    ));
    session.set_speaker(setup.speaker.clone());
    let r = reply(
        &session
            .send(request(
                "Keep the speaker in frame and add a heart at 0:10.",
            ))
            .await
            .unwrap(),
    );
    assert!(r["applied"].is_object(), "{r:#}");
    let now = projects.now().unwrap().project;
    let sticker = (now.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| {
            matches!(
                i.content,
                klipolo_model::timeline::ItemContent::Image { .. }
            )
        })
        .expect("the heart");
    // 9:16 is the main format: its own box. The follow holds the crop at the picture's left edge (0.316 of its width),
    // so the speaker (at 0.1 of it) sits left of the middle, at 0.316 of the frame, their head 0.22 of the width
    // either side: the sticker goes right of it.
    let b = sticker.layout.rect;
    let head_right = 0.1 / (9.0 / 16.0 / (16.0 / 9.0)) + 0.22;
    assert!(b.x >= head_right, "on the head: {b:?}");
    // 16:9 shows the whole picture: beside the head where the path now keeps the speaker (the crop's middle, 0.158 of
    // the width, 0.07 of it either side), not out at the frame's edge as before the path was known.
    let wide = sticker.layout.overrides[&Landscape];
    let path_head_right = 0.158 + 0.124 * 1080.0 / 1920.0;
    assert!(
        wide.x >= path_head_right && wide.x - path_head_right < 0.03,
        "not beside the head in 16:9: {wide:?}"
    );
}

/// The person's own picture with no place given and a follow of the speaker, in one message: the picture goes in a top
/// corner, and once the follow's path lands with it, in the corner away from a speaker near the picture's left edge,
/// never on their head.
#[tokio::test(flavor = "multi_thread")]
async fn the_persons_logo_moves_to_the_corner_away_from_the_speaker_once_the_follow_lands() {
    let mut setup = setup_with(TwoPeople {
        at_edge: true,
        ..TwoPeople::default()
    });
    setup.project.assets.push(Asset {
        id: "logo".into(),
        kind: AssetKind::Image,
        path: setup
            .dir
            .path()
            .join("logo.png")
            .to_string_lossy()
            .into_owned(),
        name: "logo.png".into(),
        duration_us: 0,
        width: 512,
        height: 512,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    });
    let (session, projects) = chat(
        &setup,
        vec![
            fc(
                "f",
                "followSubject",
                r#"{"description": "speaker", "focus": null}"#,
            ),
            fc(
                "l",
                "addImage",
                r#"{"file": "logo.png", "size": null, "position": null, "atSec": 0, "atWords": null, "durationSec": null}"#,
            ),
        ],
        "The speaker stays in frame, with your logo at the start.",
    );
    let r = reply(
        &session
            .send(request(
                "Keep the speaker in frame and put my logo at the start.",
            ))
            .await
            .unwrap(),
    );
    assert!(r["applied"].is_object(), "{r:#}");
    let now = projects.now().unwrap().project;
    let logo = (now.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| {
            matches!(
                &i.content,
                klipolo_model::timeline::ItemContent::Image { asset_id } if asset_id == "logo"
            )
        })
        .expect("the logo");
    // 9:16 is the main format: its own box. The speaker (at 0.1 of the picture) sits at 0.316 of the frame, their head
    // 0.22 of the width either side: the logo is in the right corner, inside the apps' margin.
    let b = logo.layout.rect;
    let head_right = 0.1 / (9.0 / 16.0 / (16.0 / 9.0)) + 0.22;
    assert!(b.x >= head_right, "on the head: {b:?}");
    assert!(
        b.x + b.w <= 0.94 + 1e-9 && b.y <= 0.2,
        "not in the corner: {b:?}"
    );
}
