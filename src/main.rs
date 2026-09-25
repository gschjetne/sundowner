#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use sundowner::Options;

const USAGE: &str = "\
sundowner - convert Markdown to PDF

USAGE:
    sundowner [OPTIONS] [INPUT]...

Each INPUT.md is written to INPUT.pdf next to it unless -o is given.
With no INPUT, or INPUT '-', Markdown is read from standard input.

OPTIONS:
    -o, --output <FILE>     Output file ('-' for standard output); single input only
    -p, --paper <SIZE>      a4 (default), a3, a5, letter, legal, or WIDTHxHEIGHT in mm
    -f, --font <FAMILY>     sans (default) or serif
    -s, --font-size <PT>    Body font size in points (default 11)
    -m, --margin <MM>       Page margin in millimetres (default 20)
    -t, --title <TEXT>      Document title (default: first level-1 heading)
        --no-page-numbers   Do not number pages
        --no-images         Do not load image files (images must be relative paths
                            inside the input file's directory)
    -q, --quiet             Do not print warnings
    -h, --help              Show this help
    -V, --version           Show version
";

const MM: f32 = 72.0 / 25.4;

struct Args {
    inputs: Vec<String>,
    output: Option<String>,
    options: Options,
    images: bool,
    quiet: bool,
}

fn parse_num(flag: &str, v: &str, lo: f32, hi: f32) -> Result<f32, String> {
    match v.trim().parse::<f32>() {
        Ok(x) if x.is_finite() && x >= lo && x <= hi => Ok(x),
        _ => Err(format!(
            "{flag}: expected a number between {lo} and {hi}, got '{v}'"
        )),
    }
}

fn paper(v: &str) -> Result<(f32, f32), String> {
    let (w, h) = match v.to_ascii_lowercase().as_str() {
        "a4" => (210.0, 297.0),
        "a5" => (148.0, 210.0),
        "a3" => (297.0, 420.0),
        "letter" => (215.9, 279.4),
        "legal" => (215.9, 355.6),
        other => {
            let (w, h) = other
                .split_once('x')
                .ok_or_else(|| format!("--paper: unknown size '{v}'"))?;
            (
                parse_num("--paper", w, 50.0, 5000.0)?,
                parse_num("--paper", h, 50.0, 5000.0)?,
            )
        }
    };
    Ok((w * MM, h * MM))
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        inputs: Vec::new(),
        output: None,
        options: Options::default(),
        images: true,
        quiet: false,
    };
    let mut it = std::env::args_os().skip(1);
    let mut only_files = false;
    while let Some(raw) = it.next() {
        let arg = raw.to_string_lossy().into_owned();
        if only_files || arg == "-" || !arg.starts_with('-') {
            a.inputs.push(raw.to_string_lossy().into_owned());
            continue;
        }
        let (flag, inline_val) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || -> Result<String, String> {
            if let Some(v) = inline_val.clone() {
                return Ok(v);
            }
            it.next()
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{flag}: missing value"))
        };
        match flag.as_str() {
            "--" => only_files = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("sundowner {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-o" | "--output" => a.output = Some(value()?),
            "-p" | "--paper" => {
                let (w, h) = paper(&value()?)?;
                a.options.page_width = w;
                a.options.page_height = h;
            }
            "-f" | "--font" => {
                a.options.serif = match value()?.to_ascii_lowercase().as_str() {
                    "sans" | "sans-serif" | "helvetica" => false,
                    "serif" | "times" => true,
                    v => return Err(format!("--font: expected 'sans' or 'serif', got '{v}'")),
                }
            }
            "-s" | "--font-size" => a.options.font_size = parse_num("--font-size", &value()?, 4.0, 72.0)?,
            "-m" | "--margin" => a.options.margin = parse_num("--margin", &value()?, 0.0, 100.0)? * MM,
            "-t" | "--title" => a.options.title = Some(value()?),
            "--no-page-numbers" => a.options.page_numbers = false,
            "--no-images" => a.images = false,
            "-q" | "--quiet" => a.quiet = true,
            _ => return Err(format!("unknown option '{arg}' (see --help)")),
        }
    }
    // Keep a sensible text column no matter what margin and paper were chosen.
    let o = &mut a.options;
    let max_margin = (o.page_width.min(o.page_height) - 4.0 * o.font_size * 3.0) / 2.0;
    o.margin = o.margin.min(max_margin.max(0.0));
    if a.inputs.is_empty() {
        a.inputs.push("-".into());
    }
    if a.output.is_some() && a.inputs.len() > 1 {
        return Err("--output can only be used with a single input".into());
    }
    Ok(Some(a))
}

/// Write via a temporary sibling file and rename, so a failed run never
/// leaves a truncated PDF behind. The temporary file lives in the target's
/// directory, so the rename never crosses filesystems. If the directory does
/// not allow creating it (or the rename is refused), fall back to writing the
/// target directly; the whole PDF is already in memory, so only an I/O error
/// mid-write could leave a partial file.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out.pdf".into());
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let atomic = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    match atomic {
        Ok(()) => Ok(()),
        Err(first) => {
            let _ = std::fs::remove_file(&tmp);
            let direct = (|| {
                let mut f = std::fs::File::create(path)?;
                f.write_all(data)?;
                f.sync_all()
            })();
            direct.map_err(|e| {
                std::io::Error::new(e.kind(), format!("{e} (temporary file also failed: {first})"))
            })
        }
    }
}

fn convert_one(input: &str, args: &Args) -> Result<(), String> {
    let (source, base_dir, default_out) = if input == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        (buf, std::env::current_dir().ok(), None)
    } else {
        let path = PathBuf::from(input);
        let buf = std::fs::read(&path).map_err(|e| format!("{input}: {e}"))?;
        let dir = path
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    p
                }
            })
            .map(Path::to_path_buf);
        (buf, dir, Some(path.with_extension("pdf")))
    };
    let text = String::from_utf8_lossy(&source);

    let mut options = args.options.clone();
    options.base_dir = if args.images { base_dir } else { None };

    let converted = match std::panic::catch_unwind(|| sundowner::convert(&text, &options)) {
        Ok(c) => c,
        Err(_) => {
            return Err(format!(
                "{input}: internal error while converting (please report this bug)"
            ))
        }
    };
    if !args.quiet {
        for w in &converted.warnings {
            eprintln!("sundowner: warning: {input}: {w}");
        }
    }

    let out = match (&args.output, default_out) {
        (Some(o), _) => o.clone(),
        (None, Some(p)) => p.to_string_lossy().into_owned(),
        (None, None) => "-".into(),
    };
    if out == "-" {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&converted.pdf)
            .and_then(|_| stdout.flush())
            .map_err(|e| format!("writing stdout: {e}"))
    } else {
        if Path::new(&out) == Path::new(input) {
            return Err(format!("{input}: refusing to overwrite the input file"));
        }
        write_atomic(Path::new(&out), &converted.pdf).map_err(|e| format!("{out}: {e}"))
    }
}

fn main() -> ExitCode {
    // Internal errors are reported per file by convert_one; keep the default
    // panic message out of the user's terminal unless someone is debugging.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::panic::set_hook(Box::new(|_| {}));
    }
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sundowner: {e}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for input in &args.inputs {
        if let Err(e) = convert_one(input, &args) {
            eprintln!("sundowner: {e}");
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
