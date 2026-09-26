#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use sundowner::config::{self, Config};
use sundowner::fonts::{Fonts, FONT_LICENSES};
use sundowner::Options;

const USAGE: &str = "\
sundowner - convert Markdown to PDF

USAGE:
    sundowner [OPTIONS] [INPUT]...

Each INPUT.md is written to INPUT.pdf next to it unless -o is given.
With no INPUT, or INPUT '-', Markdown is read from standard input.

OPTIONS:
    -o, --output <FILE>     Output file ('-' for standard output); single input only
    -c, --config <FILE>     Settings and fonts file (default: the nearest .sundowner
                            in the input's directory or a parent directory)
        --no-config         Ignore .sundowner files
    -p, --paper <SIZE>      a4 (default), a3, a5, letter, legal, or WIDTHxHEIGHT in mm
    -s, --font-size <PT>    Body font size in points (default 11)
    -m, --margin <MM>       Page margin in millimetres (default 20)
    -t, --title <TEXT>      Document title (default: first level-1 heading)
        --no-page-numbers   Do not number pages
        --no-images         Do not load image files (images must be relative paths
                            inside the input file's directory)
    -q, --quiet             Do not print warnings
        --licenses          Show the licenses of sundowner and its bundled fonts
    -h, --help              Show this help
    -V, --version           Show version

Text is set in the bundled Alegreya and code in Cousine. Other fonts,
including fallbacks for other scripts, are added in a .sundowner file; see
the README. Installed system fonts are never used.
";

const MM: f32 = 72.0 / 25.4;

enum ConfigChoice {
    Nearest,
    File(PathBuf),
    Ignore,
}

/// Command-line settings; `None` means "use the config file or default".
struct Args {
    inputs: Vec<String>,
    output: Option<String>,
    config: ConfigChoice,
    paper: Option<(f32, f32)>,
    font_size: Option<f32>,
    margin: Option<f32>,
    title: Option<String>,
    page_numbers: Option<bool>,
    images: bool,
    quiet: bool,
}

fn licenses() -> String {
    let mut s = String::from(
        "sundowner is licensed under the MIT License.\n\n\
         The fonts bundled in this program are licensed under the SIL Open Font\n\
         License, Version 1.1. Their copyright notices and license follow.\n",
    );
    for (name, text) in FONT_LICENSES {
        s.push_str(&format!("\n==== {name} ====\n\n{text}\n"));
    }
    s
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        inputs: Vec::new(),
        output: None,
        config: ConfigChoice::Nearest,
        paper: None,
        font_size: None,
        margin: None,
        title: None,
        page_numbers: None,
        images: true,
        quiet: false,
    };
    let mut it = std::env::args_os().skip(1);
    let mut only_files = false;
    while let Some(raw) = it.next() {
        let arg = raw.to_string_lossy().into_owned();
        if only_files || arg == "-" || !arg.starts_with('-') {
            a.inputs.push(arg);
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
            "--licenses" => {
                print!("{}", licenses());
                return Ok(None);
            }
            "-o" | "--output" => a.output = Some(value()?),
            "-c" | "--config" => a.config = ConfigChoice::File(PathBuf::from(value()?)),
            "--no-config" => a.config = ConfigChoice::Ignore,
            "-p" | "--paper" => a.paper = Some(config::parse_paper(&value()?).map_err(|e| format!("--{e}"))?),
            "-s" | "--font-size" => {
                a.font_size = Some(config::parse_number("--font-size", &value()?, 4.0, 72.0)?)
            }
            "-m" | "--margin" => a.margin = Some(config::parse_number("--margin", &value()?, 0.0, 100.0)?),
            "-t" | "--title" => a.title = Some(value()?),
            "--no-page-numbers" => a.page_numbers = Some(false),
            "--no-images" => a.images = false,
            "-q" | "--quiet" => a.quiet = true,
            _ => return Err(format!("unknown option '{arg}' (see --help)")),
        }
    }
    if a.inputs.is_empty() {
        a.inputs.push("-".into());
    }
    if a.output.is_some() && a.inputs.len() > 1 {
        return Err("--output can only be used with a single input".into());
    }
    Ok(Some(a))
}

/// Loaded configurations and their fonts, keyed by config file (None = no
/// config), so fonts are parsed once per run.
type ConfigCache = HashMap<Option<PathBuf>, Result<(Config, Arc<Fonts>), String>>;

fn load_config(
    path: Option<PathBuf>,
    cache: &mut ConfigCache,
    quiet: bool,
) -> Result<(Config, Arc<Fonts>), String> {
    cache
        .entry(path.clone())
        .or_insert_with(|| match &path {
            None => Ok((Config::default(), Fonts::builtin())),
            Some(p) => {
                let cfg = config::load(p)?;
                let fonts = Fonts::load(&cfg.fonts).map_err(|e| format!("{}: {e}", p.display()))?;
                if !quiet {
                    for w in &fonts.warnings {
                        eprintln!("sundowner: warning: {w}");
                    }
                }
                Ok((cfg, Arc::new(fonts)))
            }
        })
        .clone()
}

fn build_options(args: &Args, cfg: &Config, fonts: Arc<Fonts>, base_dir: Option<PathBuf>) -> Options {
    let mut o = Options {
        fonts,
        ..Options::default()
    };
    if let Some((w, h)) = args.paper.or(cfg.paper) {
        o.page_width = w;
        o.page_height = h;
    }
    if let Some(s) = args.font_size.or(cfg.font_size) {
        o.font_size = s;
    }
    if let Some(m) = args.margin.or(cfg.margin) {
        o.margin = m * MM;
    }
    o.page_numbers = args.page_numbers.or(cfg.page_numbers).unwrap_or(true);
    o.title = args.title.clone();
    o.base_dir = if args.images { base_dir } else { None };
    // Keep a sensible text column no matter what margin and paper were chosen.
    let max_margin = (o.page_width.min(o.page_height) - 12.0 * o.font_size) / 2.0;
    o.margin = o.margin.min(max_margin.max(0.0));
    o
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

fn convert_one(input: &str, args: &Args, cache: &mut ConfigCache) -> Result<(), String> {
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

    let config_path = match &args.config {
        ConfigChoice::Ignore => None,
        ConfigChoice::File(p) => Some(p.clone()),
        ConfigChoice::Nearest => base_dir.as_deref().and_then(config::find),
    };
    let (cfg, fonts) = load_config(config_path, cache, args.quiet)?;
    let options = build_options(args, &cfg, fonts, base_dir);

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
    let mut cache = ConfigCache::new();
    let mut failed = false;
    for input in &args.inputs {
        if let Err(e) = convert_one(input, &args, &mut cache) {
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
