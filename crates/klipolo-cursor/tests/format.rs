use klipolo_cursor::ClickSource;
use klipolo_cursor::format::{Capture, Line, MAX_LINE, SystemShape, parse_line, write_line};

fn shape(sys: Option<SystemShape>) -> Line {
    Line::ShapeDef {
        id: 3,
        key: "0123456789abcdef".into(),
        file: "cursors/3.png".into(),
        pw: 64,
        ph: 64,
        hx: 4.5,
        hy: 2.0,
        pts: 8.0,
        sys,
    }
}

#[test]
fn every_line_kind_round_trips() {
    let lines = [
        Line::Header {
            v: 2,
            capture: Capture::Separate,
            clicks: ClickSource::Events,
            w: 1440.0,
            h: 900.0,
        },
        Line::Move {
            t: 4_000,
            x: -0.25,
            y: 1.25,
        },
        Line::Button {
            t: 5_000,
            btn: 2,
            d: 1,
            x: 0.5,
            y: 0.5,
        },
        shape(Some(SystemShape::IBeam)),
        shape(None),
        Line::Shape { t: 6_000, id: 3 },
        Line::Visible {
            t: 7_000,
            on: false,
        },
        Line::Area {
            t: 8_000,
            w: 800.0,
            h: 600.0,
        },
        Line::Clicks {
            t: 9_000,
            clicks: ClickSource::Sampled,
        },
        Line::Burned { t: 10_000 },
    ];
    for line in lines {
        let mut out = Vec::new();
        write_line(&line, &mut out);
        assert_eq!(out.last(), Some(&b'\n'));
        let text = &out[..out.len() - 1];
        assert_eq!(
            parse_line(text),
            Some(line),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn a_torn_foreign_overlong_or_non_finite_line_is_none() {
    let long = format!(
        r#"{{"k":"m","t":1,"x":0.5,"y":0.5{}}}"#,
        " ".repeat(MAX_LINE)
    );
    let bad = [
        r#"{"k":"m","t":1,"x":0.5"#,            // torn
        r#"{"k":"q","t":1}"#,                   // a kind this build doesn't know
        r#"{"t":"durable","through":{}}"#,      // another file's line
        "\0\0\0\0",                             // a power cut's zeros
        long.as_str(),                          // over-long
        r#"{"k":"m","t":1,"x":1e39,"y":0.5}"#,  // not finite as an f32
        r#"{"k":"m","t":1.5,"x":0.5,"y":0.5}"#, // a time that isn't whole µs
        r#"{"k":"h","v":3,"capture":"separate","clicks":"events","w":1,"h":1}"#, // another format
        r#"{"k":"b","t":1,"btn":0,"d":2,"x":0.5,"y":0.5}"#, // neither down nor up
        r#"{"k":"S","id":1,"key":"12","file":"cursors/1.png","pw":8,"ph":8,"hx":0,"hy":0,"pts":8}"#, // short key
        r#"{"k":"S","id":1,"key":"0123456789abcdef","file":"cursors/123456.png","pw":8,"ph":8,"hx":0,"hy":0,"pts":8}"#,
        r#"{"k":"S","id":1,"key":"0123456789abcdef","file":"/etc/cursors/1.png","pw":8,"ph":8,"hx":0,"hy":0,"pts":8}"#,
        r#"{"k":"S","id":1,"key":"0123456789abcdef","file":"cursors/1.png","pw":257,"ph":8,"hx":0,"hy":0,"pts":8}"#,
    ];
    for line in bad {
        assert_eq!(parse_line(line.as_bytes()), None, "{line}");
    }
}

#[test]
fn a_version_one_line_is_not_a_format_two_line() {
    // Format 1 has no `k`: never a `Line`. The track still reads it as positions and presses
    // `track::a_version_one_file_keeps_its_positions_and_presses`).
    assert_eq!(parse_line(br#"{"t":16667,"x":0.5,"y":0.25,"b":1}"#), None);
}
