//! The render worker in the tests: the worker's loop driven over in-memory pipes with an injected
//! meter (`make::worker::serve_with`), on a thread of its own as the real worker runs, in a session whose
//! `in/fonts.json` names the bundled fonts; what it answered, and what its meter timed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use klipolo_engine::make::fonts::{WorkerFonts, write_fonts};
use klipolo_engine::make::measure::{FakeMeter, Metered};
use klipolo_engine::make::protocol::{
    DraftSpec, FrameSource, FramesSpec, MeasureSpec, WireKind, WorkerReply, WorkerRequest,
};
use klipolo_engine::make::session::{IN, Session};
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::project::FormatKind;
use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

mod meter;
mod project;
mod replies;
mod scenes;
pub use replies::*;
#[allow(unused_imports)]
pub use scenes::*;

/// The render worker's loop in this process, on a thread of its own, with `FakeMeter`.
pub struct InProcessWorker {
    input: DuplexStream,
    lines: UnboundedReceiver<Vec<u8>>,
    pending: Vec<u8>,
    log: Arc<Mutex<Vec<Metered>>>,
    session: PathBuf,
    _root: tempfile::TempDir,
    next_id: u64,
    failed: usize,
    notes: Vec<String>,
}

struct LineOut(UnboundedSender<Vec<u8>>);

impl std::io::Write for LineOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(std::io::Error::other)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl InProcessWorker {
    pub async fn new(meter: FakeMeter) -> Self {
        let root = tempfile::tempdir().unwrap();
        let session = Session::create(root.path()).unwrap().dir().to_owned();
        let fonts = WorkerFonts {
            bundled: klipolo_fonts::bundled::bundled_dir(),
            downloaded: Vec::new(),
        };
        write_fonts(&session.join(IN), &fonts).unwrap();
        let (input, worker_input) = tokio::io::duplex(1 << 20);
        let (out, lines) = tokio::sync::mpsc::unbounded_channel();
        let log = meter.log();
        let dir = session.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(klipolo_engine::make::worker::serve_with(
                Box::new(meter),
                worker_input,
                LineOut(out),
                &dir,
            ))
        });
        let mut w = Self {
            input,
            lines,
            pending: Vec::new(),
            log,
            session,
            _root: root,
            next_id: 0,
            failed: 0,
            notes: Vec::new(),
        };
        assert!(w.line().await.contains("\"ready\""));
        w
    }

    async fn line(&mut self) -> String {
        loop {
            if let Some(at) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=at).collect();
                return String::from_utf8(line[..line.len() - 1].to_vec()).unwrap();
            }
            let chunk = self.lines.recv().await.expect("the worker ended");
            self.pending.extend(chunk);
        }
    }

    /// Every line of the reply to `request` (`id` given here), up to its final one.
    pub async fn request(&mut self, make: impl FnOnce(u64) -> WorkerRequest) -> Vec<String> {
        self.next_id += 1;
        let line = serde_json::to_string(&make(self.next_id)).unwrap() + "\n";
        self.input.write_all(line.as_bytes()).await.unwrap();
        let mut lines = Vec::new();
        loop {
            let line = self.line().await;
            let reply: WorkerReply =
                serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
            lines.push(line);
            if let WorkerReply::Failed { .. } = reply {
                self.failed += 1;
            }
            if let WorkerReply::Framed { notes, .. } = &reply {
                self.notes = notes.clone();
            }
            if reply.is_final() {
                return lines;
            }
        }
    }

    /// `Verify` of `id` from `files`, written into the session's `in/` as the app writes them: every line of its reply.
    /// of its reply.
    pub async fn verify(
        &mut self,
        id: &klipolo_model::contrib::ContribId,
        files: &[(String, Vec<u8>)],
    ) -> Vec<String> {
        use klipolo_engine::make::protocol::{InputName, Key};
        let folder = InputName::new("contrib-verify").unwrap();
        let dir = self.session.join(IN).join(folder.as_str());
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let contrib = id.clone();
        self.request(move |n| WorkerRequest::Verify {
            id: n,
            contrib,
            folder,
            key: Key([0; 32]),
            measure: MeasureSpec::default(),
            contribs: None,
        })
        .await
    }

    pub async fn compile(&mut self, def: ContribDef, body: &str) -> CompileReply {
        self.compile_with(def, body, MeasureSpec::default()).await
    }

    /// A compile whose measurement is asked for with `spec`.
    pub async fn compile_with(
        &mut self,
        def: ContribDef,
        body: &str,
        spec: MeasureSpec,
    ) -> CompileReply {
        let kind = match def.kind {
            ContribKind::Effect => WireKind::Filter,
            ContribKind::Transition => WireKind::Transition,
            _ => WireKind::Generator,
        };
        let draft = DraftSpec {
            def: def.clone(),
            body: body.into(),
            params: BTreeMap::new(),
        };
        let lines = self
            .request(|id| WorkerRequest::Compile {
                id,
                kind,
                def,
                body: body.into(),
                measure: spec,
            })
            .await;
        let compiled = lines
            .iter()
            .find_map(|l| match serde_json::from_str(l).ok()? {
                WorkerReply::Compiled {
                    ok,
                    cost,
                    diagnostics,
                    ..
                } => Some(CompiledView {
                    ok,
                    cost,
                    diagnostics,
                    draft: draft.clone(),
                }),
                _ => None,
            });
        CompileReply { lines, compiled }
    }

    /// `c`'s draft alone at `times`, `long_edge` px.
    pub async fn frames_alone(
        &mut self,
        c: &CompiledView,
        times: &[f64],
        long_edge: u32,
    ) -> Vec<FrameView> {
        let source = FrameSource::Alone {
            draft: Box::new(c.draft.clone()),
            contribs: None,
        };
        self.frames(source, times, long_edge).await
    }

    /// Every reply line to frames of `c`'s draft alone (a refusal included).
    pub async fn frames_alone_lines(
        &mut self,
        c: &CompiledView,
        times: &[f64],
        long_edge: u32,
    ) -> Vec<String> {
        let spec = FramesSpec {
            source: FrameSource::Alone {
                draft: Box::new(c.draft.clone()),
                contribs: None,
            },
            times: times.to_vec(),
            format: FormatKind::Landscape,
            long_edge,
            sheet: false,
            busy: false,
        };
        self.request(|id| WorkerRequest::Frames { id, spec }).await
    }

    pub(crate) async fn frames(
        &mut self,
        source: FrameSource,
        times: &[f64],
        long_edge: u32,
    ) -> Vec<FrameView> {
        let spec = FramesSpec {
            source,
            times: times.to_vec(),
            format: FormatKind::Landscape,
            long_edge,
            sheet: false,
            busy: false,
        };
        let lines = self.request(|id| WorkerRequest::Frames { id, spec }).await;
        lines
            .iter()
            .filter_map(|l| match serde_json::from_str(l).ok()? {
                WorkerReply::Frame { jpeg, stats, .. } => Some(FrameView {
                    stats,
                    jpeg: jpeg.into_bytes(),
                }),
                WorkerReply::Failed { reason, .. } => panic!("{reason}"),
                _ => None,
            })
            .collect()
    }

    pub fn failed_replies(&self) -> usize {
        self.failed
    }

    /// The last frames' notes.
    pub fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }
}
