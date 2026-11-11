//! Long-running work (scans, Homebrew, downloads) runs as a task the page polls.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct Task {
    pub id: u64,
    pub kind: String,
    pub title: String,
    pub status: &'static str,
    pub lines: Vec<String>,
    pub items: Vec<Value>,
    pub progress: Value,
    pub result: Value,
    pub error: Option<String>,
    pub started: u64,
    pub finished: Option<u64>,
}

#[derive(Clone)]
pub struct Ctx {
    pub task: Arc<Mutex<Task>>,
    pub stop: Arc<AtomicBool>,
}
impl Ctx {
    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        let mut t = self.task.lock().unwrap();
        crate::util::log(&t.title.clone(), line.clone());
        t.lines.push(line);
        if t.lines.len() > 4000 {
            t.lines.drain(0..1000);
        }
    }
    pub fn item(&self, v: Value) {
        self.task.lock().unwrap().items.push(v);
    }
    pub fn progress(&self, v: Value) {
        self.task.lock().unwrap().progress = v;
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

pub struct Tasks {
    map: Mutex<BTreeMap<u64, (Arc<Mutex<Task>>, Arc<AtomicBool>)>>,
    seq: AtomicU64,
}

impl Tasks {
    pub fn new() -> Tasks {
        Tasks { map: Mutex::new(BTreeMap::new()), seq: AtomicU64::new(0) }
    }

    /// Runs `f` on a worker thread. Only one task of a kind runs at a time; starting another stops the old one.
    pub fn spawn(&self, kind: &str, title: &str, f: impl FnOnce(Ctx) -> Result<Value, String> + Send + 'static) -> u64 {
        let id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let task = Arc::new(Mutex::new(Task {
            id,
            kind: kind.into(),
            title: title.into(),
            status: "running",
            lines: vec![],
            items: vec![],
            progress: Value::Null,
            result: Value::Null,
            error: None,
            started: crate::util::now_ms(),
            finished: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut m = self.map.lock().unwrap();
            for (t, s) in m.values() {
                if t.lock().unwrap().kind == kind {
                    s.store(true, Ordering::SeqCst);
                }
            }
            // keep the 30 most recent
            while m.len() > 30 {
                let k = *m.keys().next().unwrap();
                m.remove(&k);
            }
            m.insert(id, (task.clone(), stop.clone()));
        }
        let ctx = Ctx { task: task.clone(), stop };
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx.clone())));
            let mut t = task.lock().unwrap();
            t.finished = Some(crate::util::now_ms());
            match r {
                Ok(Ok(v)) => {
                    t.result = v;
                    t.status = if ctx.stopped() { "cancelled" } else { "done" };
                }
                Ok(Err(e)) => {
                    t.error = Some(e);
                    t.status = "error";
                }
                Err(_) => {
                    t.error = Some("Something went wrong.".into());
                    t.status = "error";
                }
            }
        });
        id
    }

    pub fn stop(&self, id: u64) {
        if let Some((_, s)) = self.map.lock().unwrap().get(&id) {
            s.store(true, Ordering::SeqCst);
        }
    }

    /// State plus items/lines added since the given offsets.
    pub fn view(&self, id: u64, items_from: usize, lines_from: usize) -> Option<Value> {
        let (t, _) = self.map.lock().unwrap().get(&id).cloned()?;
        let t = t.lock().unwrap();
        Some(json!({
            "id": t.id, "kind": t.kind, "title": t.title, "status": t.status, "error": t.error, "progress": t.progress,
            "result": if t.status == "running" { Value::Null } else { t.result.clone() },
            "items": t.items.iter().skip(items_from).cloned().collect::<Vec<_>>(), "itemCount": t.items.len(),
            "lines": t.lines.iter().skip(lines_from).cloned().collect::<Vec<_>>(), "lineCount": t.lines.len(),
            "started": t.started, "finished": t.finished,
        }))
    }

    pub fn running(&self) -> usize {
        self.map.lock().unwrap().values().filter(|(t, _)| t.lock().unwrap().status == "running").count()
    }
}

/// Runs a program, streaming each output line into the task log. Returns whether it succeeded.
pub fn run_logged(ctx: &Ctx, program: &str, args: &[&str], env: &[(&str, &str)]) -> bool {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    ctx.log(format!("$ {} {}", crate::util::file_name(program), args.join(" ")));
    let mut c = Command::new(program);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = match c.spawn() {
        Ok(c) => c,
        Err(e) => {
            ctx.log(format!("Couldn't start {program}: {e}"));
            return false;
        }
    };
    let err = child.stderr.take().unwrap();
    let c2 = ctx.clone();
    let t = std::thread::spawn(move || {
        for l in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            c2.log(l);
        }
    });
    let out = child.stdout.take().unwrap();
    for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
        ctx.log(l);
        if ctx.stopped() {
            let _ = child.kill();
            break;
        }
    }
    let _ = t.join();
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if ctx.stopped() {
        ctx.log("Stopped.");
    }
    ok
}
