//! The app's side of a preview worker, for the preview tests (`check_camera.rs`, `live_view.rs`, which include this file
//! by path so they link nothing of `common`'s): the app's lines, read late, and what the worker wrote, kept.

use std::collections::VecDeque;
use std::io::{BufReader, Cursor, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_capture::protocol::{Command, Event};

/// The app's lines, the first read `.1` ms late.
pub struct Late(pub Cursor<Vec<u8>>, pub u64);

impl Read for Late {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(std::mem::take(&mut self.1)));
        self.0.read(buf)
    }
}

/// The app's `cmd`, sent `ms` after the worker starts reading, then its stdin's end.
pub fn sent_after(cmd: &Command, ms: u64) -> Box<BufReader<Late>> {
    let mut line = serde_json::to_vec(cmd).unwrap();
    line.push(b'\n');
    Box::new(BufReader::new(Late(Cursor::new(line), ms)))
}

/// The app's lines, each sent `ms` after the worker starts reading, then its stdin's end.
pub struct Timed {
    lines: VecDeque<(u64, Vec<u8>)>,
    started: Option<Instant>,
    line: Cursor<Vec<u8>>,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let started = *self.started.get_or_insert_with(Instant::now);
        if self.line.position() == self.line.get_ref().len() as u64 {
            let Some((ms, line)) = self.lines.pop_front() else {
                return Ok(0);
            };
            let due = started + Duration::from_millis(ms);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            self.line = Cursor::new(line);
        }
        self.line.read(buf)
    }
}

/// The app's `cmds`, each at its time (ms after the worker starts reading), then its stdin's end.
pub fn sent_at(cmds: &[(u64, Command)]) -> Box<BufReader<Timed>> {
    let lines = (cmds.iter())
        .map(|(ms, cmd)| {
            let mut line = serde_json::to_vec(cmd).unwrap();
            line.push(b'\n');
            (*ms, line)
        })
        .collect();
    Box::new(BufReader::new(Timed {
        lines,
        started: None,
        line: Cursor::new(Vec::new()),
    }))
}

/// What the worker wrote to the app, kept for the test.
#[derive(Clone, Default)]
pub struct SharedOut(pub Arc<Mutex<Vec<u8>>>);

impl SharedOut {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    /// Each line the worker wrote, as the app reads it.
    pub fn events(&self) -> Vec<Event> {
        (self.text().lines())
            .map(|l| serde_json::from_str(l).expect("an event line"))
            .collect()
    }
}

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
