//! The speech workers' wire (moved from klipolo-speech): requests and their samples, replies and their limits.

use klipolo_speech_wire::protocol::{
    parse_reply, read_request, write_progress, write_request, Reply, Request, MAX_LANGUAGES,
    MAX_SAMPLES,
};

#[test]
fn a_request_and_its_samples_round_trip() {
    let mut wire = Vec::new();
    let req = Request::Transcribe {
        id: 7,
        samples: 3,
        language: Some("en".into()),
        prompt: "Um".into(),
        threads: 2,
    };
    write_request(&mut wire, &req, &[0.5, -0.25, 1.0]).unwrap();
    write_request(&mut wire, &Request::Abort { id: 7 }, &[]).unwrap();
    let mut r = std::io::BufReader::new(&wire[..]);
    let mut buf = Vec::new();
    assert_eq!(read_request(&mut r, &mut buf).unwrap(), Some(req));
    assert_eq!(buf, [0.5, -0.25, 1.0]);
    assert_eq!(
        read_request(&mut r, &mut buf).unwrap(),
        Some(Request::Abort { id: 7 })
    );
    assert_eq!(
        read_request(&mut r, &mut buf).unwrap(),
        None,
        "end of input"
    );
}

#[test]
fn replies_over_their_limits_are_refused() {
    use klipolo_speech_wire::protocol::{MAX_REPLY_BYTES, MAX_REPLY_WORDS};
    let words: Vec<String> = (0..MAX_REPLY_WORDS + 1)
        .map(|i| format!("{{\"text\":\"w\",\"startUs\":{i},\"endUs\":{i},\"confidence\":1}}"))
        .collect();
    let line = format!(
        "{{\"type\":\"heard\",\"id\":1,\"language\":\"en\",\"words\":[{}],\"computeMs\":1}}",
        words.join(",")
    );
    assert!(parse_reply(&line).is_none(), "too many words");
    assert!(parse_reply(&" ".repeat(MAX_REPLY_BYTES + 1)).is_none());
}

#[test]
fn a_frame_over_the_limit_is_refused() {
    let line = format!(
        "{{\"type\":\"transcribe\",\"id\":1,\"samples\":{},\"language\":null,\"prompt\":\"\",\"threads\":1}}\n",
        MAX_SAMPLES + 1
    );
    let mut buf = Vec::new();
    assert!(read_request(&mut std::io::BufReader::new(line.as_bytes()), &mut buf).is_err());
    assert!(buf.capacity() < 1024, "nothing was allocated for it");
}

#[test]
fn a_progress_line_says_how_far_its_request_is_decoded() {
    // Written by whisper's callbacks from a stack buffer, read as any reply.
    let mut line = Vec::new();
    write_progress(&mut line, u64::MAX, 0.4375).unwrap();
    let line = String::from_utf8(line).unwrap();
    assert!(line.ends_with('\n'));
    assert_eq!(
        parse_reply(&line),
        Some(Reply::Progress {
            id: u64::MAX,
            at: 0.4375
        })
    );
    let mut over = Vec::new();
    write_progress(&mut over, 1, 7.0).unwrap();
    assert_eq!(
        parse_reply(std::str::from_utf8(&over).unwrap()),
        Some(Reply::Progress { id: 1, at: 1.0 }),
        "never past its sound"
    );
    assert_eq!(
        parse_reply(r#"{"type":"progress","id":1}"#),
        None,
        "half a report is none"
    );
}

#[test]
fn a_speech_find_and_its_samples_round_trip() {
    // `findSpeech` carries the same samples frame as `transcribe`.
    let mut wire = Vec::new();
    let req = Request::FindSpeech { id: 9, samples: 2 };
    write_request(&mut wire, &req, &[0.25, -0.5]).unwrap();
    let mut r = std::io::BufReader::new(&wire[..]);
    let mut buf = Vec::new();
    assert_eq!(read_request(&mut r, &mut buf).unwrap(), Some(req));
    assert_eq!(buf, [0.25, -0.5]);
    let line = serde_json::to_string(&Reply::Speech {
        id: 9,
        spans: vec![[0, 250_000], [900_000, 1_400_000]],
    })
    .unwrap();
    assert_eq!(
        parse_reply(&line),
        Some(Reply::Speech {
            id: 9,
            spans: vec![[0, 250_000], [900_000, 1_400_000]]
        })
    );
}

#[test]
fn a_speech_reply_with_too_many_spans_is_refused() {
    use klipolo_speech_wire::protocol::MAX_REPLY_WORDS;
    let spans: Vec<String> = (0..MAX_REPLY_WORDS + 1)
        .map(|i| format!("[{i},{}]", i + 1))
        .collect();
    let line = format!(
        "{{\"type\":\"speech\",\"id\":1,\"spans\":[{}]}}",
        spans.join(",")
    );
    assert!(parse_reply(&line).is_none());
    let frame = format!(
        "{{\"type\":\"findSpeech\",\"id\":1,\"samples\":{}}}\n",
        MAX_SAMPLES + 1
    );
    let mut buf = Vec::new();
    assert!(
        read_request(&mut std::io::BufReader::new(frame.as_bytes()), &mut buf).is_err(),
        "nor a frame over the limit"
    );
}

#[test]
fn a_language_question_carries_its_samples_and_its_answer_at_most_107_scores() {
    // `language` carries the same samples frame; its answer is refused past VoxLingua107's labels.
    let mut wire = Vec::new();
    let req = Request::Language {
        id: 4,
        samples: 2,
        allowed: vec!["en".into(), "de".into()],
    };
    write_request(&mut wire, &req, &[0.25, -0.5]).unwrap();
    let mut r = std::io::BufReader::new(&wire[..]);
    let mut buf = Vec::new();
    assert_eq!(read_request(&mut r, &mut buf).unwrap(), Some(req));
    assert_eq!(buf, [0.25, -0.5]);
    // `allowedMass`: how much of the unrestricted score the allowed languages hold (a low one says the
    // speech is probably outside them).
    let answer = Reply::Language {
        id: 4,
        scores: vec![("de".into(), 0.875), ("en".into(), 0.125)],
        allowed_mass: 0.25,
    };
    assert!(serde_json::to_string(&answer)
        .unwrap()
        .contains(r#""allowedMass":0.25"#));
    let line = serde_json::to_string(&answer).unwrap();
    assert_eq!(parse_reply(&line), Some(answer));
    let many = Reply::Language {
        id: 4,
        allowed_mass: 1.0,
        scores: (0..=MAX_LANGUAGES)
            .map(|i| (format!("l{i}"), 0.0))
            .collect(),
    };
    assert_eq!(parse_reply(&serde_json::to_string(&many).unwrap()), None);
}

#[test]
fn a_long_frame_goes_out_in_parts_and_reads_back_whole() {
    // The samples go through a 64 KB buffer, never one the size of the chunk: 30 s is 30 parts after the line.
    let samples: Vec<f32> = (0..16_000 * 30).map(|i| (i % 100) as f32 / 100.0).collect();
    let req = Request::Transcribe {
        id: 1,
        samples: samples.len(),
        language: None,
        prompt: String::new(),
        threads: 1,
    };
    let mut frame = klipolo_speech_wire::protocol::Frame::new(&req, &samples);
    let mut parts = Vec::new();
    while let Some(part) = frame.next_part() {
        parts.push(part.len());
    }
    assert_eq!(parts.len(), 1 + 30, "{parts:?}");
    assert!(parts[1..].iter().all(|&n| n <= 64 << 10));
    let mut wire = Vec::new();
    write_request(&mut wire, &req, &samples).unwrap();
    let mut buf = Vec::new();
    let mut r = std::io::BufReader::new(&wire[..]);
    assert_eq!(read_request(&mut r, &mut buf).unwrap(), Some(req));
    assert_eq!(buf, samples);
}
