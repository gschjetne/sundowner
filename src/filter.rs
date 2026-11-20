//! Filters: commands that turn code blocks into images.
//!
//! sundowner draws no diagrams itself. Instead, code blocks of a given
//! language can be piped through a command, which reads the code on its
//! standard input and writes a PDF, PNG or JPEG file on its standard output.
//! The image takes the code block's place, with the code as its alt text.
//! Filters are listed in a file given with `--filters`:
//!
//! ```text
//! # Mermaid diagrams, with mermaid-cli
//! [mermaid]
//! command = mmdc --input - --output - --outputFormat png --scale 2 --quiet
//! dpi = 192
//! timeout = 60
//!
//! # Graphviz, under either name
//! [dot graphviz]
//! command = dot -Tpng -Gdpi=192
//! dpi = 192
//! ```
//!
//! Commands only ever come from that file, which the person running
//! sundowner names; never from `.sundowner` files, which travel with
//! documents, nor from documents themselves. The document reaches a command
//! only as data on its standard input: the command line is fixed. Each run
//! is limited in time and output, and the number of runs per document is
//! capped, so a hostile document can make sundowner do no more than the
//! commands it was given do with their input. Whether that is safe is up to
//! the commands (see the README).

use crate::markdown::{Block, Document, Figure};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Largest image a filter may write (as for image files).
pub const MAX_OUTPUT: usize = crate::image::MAX_FILE_BYTES as usize;
/// Most filter runs per document; code blocks beyond them stay code.
/// Identical code blocks of one language are run once.
pub const MAX_RUNS: usize = 100;
/// Most filters run at the same time.
const MAX_JOBS: usize = 4;
/// How much of a filter's standard error is kept for warnings.
const MAX_STDERR: usize = 16 << 10;

/// One filter: the command a language's code blocks are piped through.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    /// Run with `sh -c` (`cmd /C` on Windows).
    pub command: String,
    /// The resolution of the images it writes, in pixels per inch.
    pub dpi: f32,
    /// How long it may run on one code block.
    pub timeout: Duration,
}

/// Filters by language (lowercase).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    by_language: HashMap<String, Filter>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.by_language.is_empty()
    }

    /// The filter for code blocks with this info string, if any: the one
    /// for its first word, the language, ignoring case.
    pub fn for_info(&self, info: &str) -> Option<&Filter> {
        let lang = info.split_whitespace().next()?;
        self.by_language.get(&lang.to_ascii_lowercase())
    }
}

pub fn load(path: &Path) -> Result<Filters, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse a filters file: a `[language ...]` section per filter, with its
/// `command`, and optionally `dpi` (default 96) and `timeout` in seconds
/// (default 30).
pub fn parse(text: &str) -> Result<Filters, String> {
    struct Section {
        langs: Vec<String>,
        line: usize,
        command: Option<String>,
        dpi: f32,
        timeout: f32,
    }
    let mut filters = Filters::default();
    let mut add = |s: Section| {
        let command = s
            .command
            .ok_or_else(|| format!("line {}: [{}] has no command", s.line, s.langs.join(" ")))?;
        for lang in s.langs {
            let filter = Filter {
                command: command.clone(),
                dpi: s.dpi,
                timeout: Duration::from_secs_f32(s.timeout),
            };
            if filters.by_language.insert(lang.clone(), filter).is_some() {
                return Err(format!("line {}: '{lang}' has a filter already", s.line));
            }
        }
        Ok(())
    };
    let mut section: Option<Section> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let err = |m: String| format!("line {}: {m}", n + 1);
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(s) = section.take() {
                add(s)?;
            }
            let langs: Vec<String> = name
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|l| !l.is_empty())
                .map(str::to_ascii_lowercase)
                .collect();
            if langs.is_empty() {
                return Err(err(
                    "a section names the languages it is for, as in [mermaid]".into()
                ));
            }
            section = Some(Section {
                langs,
                line: n + 1,
                command: None,
                dpi: 96.0,
                timeout: 30.0,
            });
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err("expected 'key = value'".into()))?;
        let (key, value) = (key.trim(), value.trim());
        let s = section
            .as_mut()
            .ok_or_else(|| err(format!("'{key}' outside a [language] section")))?;
        match key {
            "command" if value.is_empty() => return Err(err("command: missing command".into())),
            "command" => s.command = Some(value.to_string()),
            "dpi" => s.dpi = crate::config::parse_number("dpi", value, 10.0, 2400.0).map_err(err)?,
            "timeout" => {
                s.timeout = crate::config::parse_number("timeout", value, 0.1, 3600.0).map_err(err)?
            }
            _ => {
                return Err(err(format!(
                    "unknown setting '{key}' (use command, dpi, timeout)"
                )))
            }
        }
    }
    if let Some(s) = section {
        add(s)?;
    }
    Ok(filters)
}

/// Run the filters on the code blocks they are for, replacing each that
/// gives an image with a [`Block::Figure`]. Code blocks whose filter fails
/// stay code, with a warning; so do those beyond [`MAX_RUNS`].
pub fn apply(doc: &mut Document, filters: &Filters) -> Vec<String> {
    let mut warnings = Vec::new();
    if filters.is_empty() {
        return warnings;
    }
    // The distinct (language, code) pairs to run, in document order, and
    // the job each code block needs, in the order `code_blocks` visits them.
    let mut jobs: Vec<Job> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    let mut plan: Vec<Option<usize>> = Vec::new();
    // How many code blocks of each language there have been so far.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut skipped = 0;
    code_blocks(&mut doc.blocks, &mut |block| {
        let Block::Code { info, lines } = block else {
            return;
        };
        let job = filters.for_info(info).and_then(|filter| {
            let lang = info.split_whitespace().next()?.to_ascii_lowercase();
            let mut code = lines.join("\n");
            code.push('\n');
            let ordinal = seen.entry(lang.clone()).or_default();
            *ordinal += 1;
            let ordinal = *ordinal;
            let key = (lang, code);
            if let Some(&k) = index.get(&key) {
                return Some(k);
            }
            if jobs.len() == MAX_RUNS {
                skipped += 1;
                return None;
            }
            jobs.push(Job {
                lang: key.0.clone(),
                ordinal,
                code: key.1.clone(),
                filter: filter.clone(),
            });
            index.insert(key, jobs.len() - 1);
            Some(jobs.len() - 1)
        });
        plan.push(job);
    });
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} code block(s) are shown as code, as only {MAX_RUNS} different ones \
             are run through filters"
        ));
    }

    // The figure each job made, if it succeeded.
    let mut figures = Vec::with_capacity(jobs.len());
    for (job, result) in jobs.iter().zip(run_all(&jobs)) {
        let checked = result.and_then(|data| {
            let loaded = if crate::import::is_pdf(&data) {
                crate::import::load(&data).map(|form| {
                    if form.pages > 1 {
                        warnings.push(format!(
                            "{} code block {}: only the first of the {} pages its filter wrote is shown",
                            job.lang, job.ordinal, form.pages
                        ));
                    }
                })
            } else {
                crate::image::load(&data).map(|_| ())
            };
            match loaded {
                Ok(()) => Ok(data),
                Err(e) => Err(format!(
                    "it did not write a PDF, PNG or JPEG file it can embed ({e})"
                )),
            }
        });
        match checked {
            Ok(data) => {
                figures.push(Some(doc.figures.len()));
                doc.figures.push(Figure {
                    data,
                    dpi: job.filter.dpi,
                });
            }
            Err(e) => {
                figures.push(None);
                warnings.push(format!(
                    "{} code block {}: the filter failed, so it is shown as code: {e}",
                    job.lang, job.ordinal
                ));
            }
        }
    }
    let mut plan = plan.into_iter();
    code_blocks(&mut doc.blocks, &mut |block| {
        let Some(Some(index)) = plan.next().flatten().map(|job| figures[job]) else {
            return;
        };
        if let Block::Code { lines, .. } = block {
            let lines = std::mem::take(lines);
            *block = Block::Figure { index, lines };
        }
    });
    warnings
}

/// Call `f` on every code block, in quotes and lists too, in order.
fn code_blocks(blocks: &mut [Block], f: &mut dyn FnMut(&mut Block)) {
    for b in blocks {
        match b {
            Block::Code { .. } => f(b),
            Block::Quote(inner) => code_blocks(inner, f),
            Block::List { items, .. } => {
                for item in items {
                    code_blocks(&mut item.blocks, f);
                }
            }
            _ => {}
        }
    }
}

struct Job {
    lang: String,
    /// Which code block of its language this is (the first, if several are
    /// the same), for warnings.
    ordinal: usize,
    code: String,
    filter: Filter,
}

/// Run the jobs, a few at a time, and return their outputs in order. Once
/// they add up to more than the images of a document may, the rest fail.
fn run_all(jobs: &[Job]) -> Vec<Result<Vec<u8>, String>> {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_JOBS)
        .min(jobs.len());
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    let too_much = || {
        format!(
            "the images filters made add up to more than {} MiB",
            crate::image::MAX_TOTAL_BYTES >> 20
        )
    };
    let mut results: Vec<Option<Result<Vec<u8>, String>>> = (0..jobs.len()).map(|_| None).collect();
    let done = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(k) else {
                            return mine;
                        };
                        if total.load(Ordering::Relaxed) > crate::image::MAX_TOTAL_BYTES {
                            mine.push((k, Err(too_much())));
                            continue;
                        }
                        let r = run(&job.filter.command, job.code.as_bytes(), job.filter.timeout);
                        let r = r.and_then(|data| {
                            let sum = total.fetch_add(data.len(), Ordering::Relaxed) + data.len();
                            if sum > crate::image::MAX_TOTAL_BYTES {
                                Err(too_much())
                            } else {
                                Ok(data)
                            }
                        });
                        mine.push((k, r));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect::<Vec<_>>()
    });
    for (k, r) in done {
        results[k] = Some(r);
    }
    results
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err("internal error".into())))
        .collect()
}

/// The shell that runs a command line.
fn shell(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// Run `command` with `input` on its standard input, and return what it
/// writes on its standard output: at most [`MAX_OUTPUT`] bytes, within
/// `timeout`. If it takes longer, or writes more, it is killed, and with it
/// (on Unix) every process it started, as they share its process group.
pub fn run(command: &str, input: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let mut cmd = shell(command);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run it: {e}"))?;

    // Feed and drain the pipes on threads of their own, so that a command
    // that writes before it has read everything cannot block on us, and
    // that the deadline holds whatever the command does with its pipes. The
    // threads end when the pipes close, at the latest when the command and
    // what it started are killed.
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    if let Some(mut stdin) = stdin {
        let input = input.to_vec();
        std::thread::spawn(move || {
            use std::io::Write;
            // A command may well stop reading early; that is not an error.
            let _ = stdin.write_all(&input);
        });
    }
    let (out_tx, out_rx) = mpsc::channel();
    if let Some(stdout) = stdout {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let r = stdout.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut buf);
            let _ = out_tx.send(r.map(|_| buf));
        });
    }
    let (err_tx, err_rx) = mpsc::channel();
    if let Some(mut stderr) = stderr {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let room = MAX_STDERR.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
            let _ = err_tx.send(kept);
        });
    }
    let stderr_text = || {
        let bytes = err_rx
            .recv_timeout(Duration::from_millis(200))
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        text.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| {
                let mut l: String = l.chars().take(300).collect();
                l.insert_str(0, ": ");
                l
            })
            .unwrap_or_default()
    };

    let remaining = || deadline.saturating_duration_since(Instant::now());
    let output = match out_rx.recv_timeout(remaining()) {
        Ok(Ok(out)) if out.len() > MAX_OUTPUT => {
            kill(&mut child);
            return Err(format!("it wrote more than {} MiB", MAX_OUTPUT >> 20));
        }
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            kill(&mut child);
            return Err(format!("reading its output: {e}"));
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill(&mut child);
            return Err(format!("it took longer than {} s", timeout.as_secs_f32()));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            kill(&mut child);
            return Err("its output could not be read".into());
        }
    };
    // Its output is complete; wait for it to exit.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if remaining().is_zero() => {
                kill(&mut child);
                return Err(format!("it took longer than {} s", timeout.as_secs_f32()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                kill(&mut child);
                return Err(e.to_string());
            }
        }
    };
    if !status.success() {
        let how = match status.code() {
            Some(code) => format!("it exited with status {code}"),
            None => "it was killed".to_string(),
        };
        return Err(format!("{how}{}", stderr_text()));
    }
    if output.is_empty() {
        return Err(format!("it wrote nothing{}", stderr_text()));
    }
    Ok(output)
}

/// Kill a command that has not been waited for, and on Unix everything in
/// its process group. Its process ID (the group's ID) cannot have been
/// reused, as it has not been reaped yet.
fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-s", "KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
