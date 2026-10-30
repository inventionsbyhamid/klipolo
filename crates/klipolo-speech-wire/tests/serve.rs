//! One loop for every engine, each with its own state: `serve` on a thread over two `std::io::pipe`s, as a
//! worker runs it over stdin and stdout.

use std::io::{self, BufRead, BufReader};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use klipolo_speech_wire::engine::{serve, EngineAsk, Job, Languages, SpeechEngine, Transcribed};
use klipolo_speech_wire::protocol::{parse_reply, write_request, Reply, Request};

/// Works 1 s per request in 5 ms steps, reporting each step and stopping when asked.
struct Slow;

impl SpeechEngine for Slow {
    fn transcribe(&mut self, ask: &EngineAsk<'_>, job: &Job<'_>) -> Result<Transcribed, String> {
        for step in 0..200 {
            if job.stopped() {
                return Err("stopped".into());
            }
            job.progress(step as f32 / 200.0);
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Transcribed {
            language: ask.language.unwrap_or("en").into(),
            words: vec![],
        })
    }
}

/// `serve` on a thread: requests go in through one pipe, replies come out of the other, read on a thread of their
/// own so a test never waits for ever.
struct Wired {
    to: Mutex<Option<io::PipeWriter>>,
    replies: Mutex<mpsc::Receiver<Reply>>,
    ready: Reply,
    served: Mutex<Option<JoinHandle<i32>>>,
}

impl Wired {
    fn start(engine: impl SpeechEngine + Send + 'static) -> Self {
        Self::start_with(engine, || Ok(()))
    }

    fn start_with(
        mut engine: impl SpeechEngine + Send + 'static,
        lower: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) -> Self {
        let (input, to) = io::pipe().unwrap();
        let (from, output) = io::pipe().unwrap();
        let served = std::thread::spawn(move || {
            serve(
                &mut engine,
                "test.gguf",
                lower,
                BufReader::new(input),
                output,
            )
        });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(from).lines() {
                let Ok(line) = line else { break };
                if let Some(r) = parse_reply(&line) {
                    if tx.send(r).is_err() {
                        break;
                    }
                }
            }
        });
        let ready = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker said something");
        Self {
            to: Mutex::new(Some(to)),
            replies: Mutex::new(rx),
            ready,
            served: Mutex::new(Some(served)),
        }
    }

    fn ready(&self) -> Reply {
        self.ready.clone()
    }

    fn send(&self, req: &Request, samples: &[f32]) {
        let mut to = self.to.lock().unwrap();
        write_request(to.as_mut().expect("open"), req, samples).unwrap();
    }

    fn next(&self) -> Reply {
        self.replies
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .expect("a reply within 10 s")
    }

    /// The first reply after those already read that `done` takes.
    fn until(&self, done: impl Fn(&Reply) -> bool) -> Reply {
        loop {
            let r = self.next();
            if done(&r) {
                return r;
            }
        }
    }

    /// Every reply after those already read, up to and with the first that `done` takes.
    fn all_until(&self, done: impl Fn(&Reply) -> bool) -> Vec<Reply> {
        let mut all = Vec::new();
        loop {
            let r = self.next();
            let last = done(&r);
            all.push(r);
            if last {
                return all;
            }
        }
    }

    /// Closes the loop's stdin and waits for it to end: its exit code.
    fn close_and_wait(&self) -> i32 {
        drop(self.to.lock().unwrap().take());
        let served = self.served.lock().unwrap().take().expect("running");
        served.join().unwrap()
    }
}

fn transcribe(id: u64) -> Request {
    Request::Transcribe {
        id,
        samples: 16_000,
        language: Some("en".into()),
        prompt: String::new(),
        threads: 2,
    }
}

#[test]
fn an_abort_for_the_request_in_flight_stops_it_and_one_for_another_id_does_not() {
    let w = Wired::start(Slow);
    w.send(
        &Request::Transcribe {
            id: 7,
            samples: 16_000,
            language: None,
            prompt: String::new(),
            threads: 2,
        },
        &[0.0; 16_000],
    );
    w.send(&Request::Abort { id: 6 }, &[]);
    std::thread::sleep(Duration::from_millis(100));
    w.send(&Request::Abort { id: 7 }, &[]);
    assert_eq!(
        w.until(|r| !matches!(r, Reply::Progress { .. })),
        Reply::Aborted { id: 7 }
    );
}

#[test]
fn progress_goes_out_at_most_four_times_a_second_starting_at_once() {
    let w = Wired::start(Slow);
    w.send(&transcribe(1), &[0.0; 16_000]);
    let replies = w.all_until(|r| matches!(r, Reply::Heard { .. }));
    assert_eq!(
        replies[0],
        Reply::Progress { id: 1, at: 0.0 },
        "said as it begins"
    );
    let progress = replies
        .iter()
        .filter(|r| matches!(r, Reply::Progress { id: 1, .. }))
        .count();
    assert!(
        (2..=5).contains(&progress),
        "{progress} lines for 1 s of work"
    );
}

#[test]
fn ready_comes_after_lowering_and_a_refused_lowering_still_says_ready() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let o = order.clone();
    let w = Wired::start_with(Slow, move || {
        o.lock().unwrap().push("lowered");
        Err(io::Error::other("refused"))
    });
    assert!(matches!(w.ready(), Reply::Ready { .. }));
    assert_eq!(*order.lock().unwrap(), ["lowered"]);
}

#[test]
fn an_engine_without_vad_or_a_language_finder_fails_only_that_request() {
    let w = Wired::start(Slow);
    w.send(
        &Request::FindSpeech {
            id: 3,
            samples: 160,
        },
        &[0.0; 160],
    );
    w.send(
        &Request::Language {
            id: 4,
            samples: 160,
            allowed: vec!["en".into()],
        },
        &[0.0; 160],
    );
    w.send(&transcribe(5), &[0.0; 16_000]);
    let r = w.all_until(|r| matches!(r, Reply::Heard { id: 5, .. }));
    assert!(
        r.iter().any(|r| matches!(r, Reply::Failed { id: 3, .. }))
            && r.iter().any(|r| matches!(r, Reply::Failed { id: 4, .. }))
    );
}

#[test]
fn closing_stdin_stops_the_request_in_flight_and_ends_the_loop_without_touching_another_loop() {
    let (a, b) = (Wired::start(Slow), Wired::start(Slow));
    a.send(&transcribe(9), &[0.0; 16_000]);
    b.send(&transcribe(9), &[0.0; 16_000]);
    assert_eq!(a.close_and_wait(), 0);
    assert!(
        matches!(
            b.until(|r| !matches!(r, Reply::Progress { .. })),
            Reply::Heard { id: 9, .. }
        ),
        "b's state is its own"
    );
}

#[test]
fn a_request_aborted_while_it_waited_never_reaches_the_engine() {
    // Pause, resume and pause within one look: request 2 waits behind 1 and is aborted
    // there, so it answers `aborted` without running, and 3 runs.
    struct Counting(Arc<Mutex<Vec<u64>>>);
    impl SpeechEngine for Counting {
        fn transcribe(&mut self, _: &EngineAsk<'_>, job: &Job<'_>) -> Result<Transcribed, String> {
            self.0.lock().unwrap().push(job.id());
            std::thread::sleep(Duration::from_millis(300));
            Ok(Transcribed {
                language: String::new(),
                words: vec![],
            })
        }
    }
    let ran = Arc::new(Mutex::new(Vec::new()));
    let w = Wired::start(Counting(ran.clone()));
    w.send(&transcribe(1), &[0.0; 16_000]);
    w.send(&transcribe(2), &[0.0; 16_000]);
    w.send(&Request::Abort { id: 2 }, &[]);
    w.send(&transcribe(3), &[0.0; 16_000]);
    let r = w.all_until(|r| matches!(r, Reply::Heard { id: 3, .. }));
    assert!(r.contains(&Reply::Aborted { id: 2 }), "{r:?}");
    assert_eq!(*ran.lock().unwrap(), [1, 3]);
}

#[test]
fn a_language_answer_carries_the_allowed_languages_share() {
    // The finder's scores are renormalised over the allowed languages, so only `allowed_mass` says the
    // speech is probably outside them (the "unsure"). It goes out with the scores, which keep the engine's order.
    struct Finder;
    impl SpeechEngine for Finder {
        fn transcribe(&mut self, _: &EngineAsk<'_>, _: &Job<'_>) -> Result<Transcribed, String> {
            Err("a finder only".into())
        }
        fn languages(
            &mut self,
            _: &[f32],
            allowed: &[String],
            _: &Job<'_>,
        ) -> Option<Result<Languages, String>> {
            Some(Ok(Languages {
                scores: allowed.iter().map(|l| (l.clone(), 0.5)).collect(),
                allowed_mass: 0.15,
            }))
        }
    }
    let w = Wired::start(Finder);
    w.send(
        &Request::Language {
            id: 2,
            samples: 160,
            allowed: vec!["de".into(), "en".into()],
        },
        &[0.0; 160],
    );
    assert_eq!(
        w.until(|r| !matches!(r, Reply::Progress { .. })),
        Reply::Language {
            id: 2,
            scores: vec![("de".into(), 0.5), ("en".into(), 0.5)],
            allowed_mass: 0.15,
        }
    );
}
