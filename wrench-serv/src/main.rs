//! Service for uploading a program with a configuration, running it (and the
//! test cases of a variant) through the `wrench` binary, and showing the report.

mod code_view;
mod statistics;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};
use uuid::Uuid;

use code_view::{escape_html, format_asm, format_code_with_line_numbers, format_report};
use statistics::{get_posthog_id_from_cookie, get_track, post_hog_tracker, track_cookie, track_event, Event};
use wrench::config::execution_stats_report;
use wrench::isa::Isa;

#[derive(Debug)]
struct Config {
    port: u16,
    wrench_path: String,
    wrench_args: Vec<String>,
    storage_path: String,
    variants_path: String,
    examples_path: String,
    log_limit: usize,
    variants: Vec<String>,
}

fn env_or(name: &str, def: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| def.to_string())
}

fn init_config() -> Config {
    let exec = env_or("WRENCH_EXEC", "wrench");
    let mut parts = exec.split(' ').map(String::from);
    let wrench_path = parts.next().unwrap_or_default();
    let variants_path = env_or("VARIANTS", "variants");
    let mut variants: Vec<String> = fs::read_dir(&variants_path)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    variants.sort();
    Config {
        port: env_or("PORT", "8080").parse().expect("PORT should be a number"),
        wrench_path,
        wrench_args: parts.collect(),
        storage_path: env_or("STORAGE_PATH", "uploads"),
        variants_path,
        examples_path: env_or("EXAMPLES_PATH", "examples"),
        log_limit: env_or("LOG_LIMIT", "10000")
            .parse()
            .expect("LOG_LIMIT should be a number"),
        variants,
    }
}

/// Substitute `{{key}}` placeholders in one pass: inserted values are never
/// scanned again, and unknown placeholders are kept as is.
fn fill_template(vars: &[(&str, String)], template: &str) -> String {
    let mut out = String::new();
    let mut t = template;
    while let Some(open) = t.find("{{") {
        out.push_str(&t[..open]);
        let rest = &t[open + 2..];
        match rest.find("}}") {
            Some(close) if vars.iter().any(|(k, _)| *k == &rest[..close]) => {
                let key = &rest[..close];
                out.push_str(
                    &vars
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default(),
                );
                t = &rest[close + 2..];
            }
            _ => {
                out.push_str("{{");
                t = rest;
            }
        }
    }
    out.push_str(t);
    out
}

const ASSETS_DIR: &str = "static/assets";

/// Bytes as hex digits without padding, as the original version stamps were built.
fn hex_unpadded(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:x}", b)).collect()
}

fn sha1_hex(data: &[u8]) -> String {
    hex_unpadded(&sha1_smol::Sha1::from(data).digest().bytes())
}

/// A page template with `{{asset:<file>}}` replaced by versioned asset URLs.
fn read_template(path: &str) -> std::io::Result<String> {
    let template = fs::read_to_string(path)?;
    let mut versions = Vec::new();
    for entry in fs::read_dir(ASSETS_DIR)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let hash = sha1_hex(&fs::read(entry.path())?);
        let url = format!("/assets/{}?v={}", name, hash.chars().take(12).collect::<String>());
        versions.push((format!("asset:{}", name), url));
    }
    let vars: Vec<(&str, String)> = versions.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    Ok(fill_template(&vars, &template))
}

type Resp = Response<std::io::Cursor<Vec<u8>>>;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn html(body: String) -> Resp {
    Response::from_string(body).with_header(header("Content-Type", "text/html;charset=utf-8"))
}

fn redirect(location: &str) -> Resp {
    Response::from_string("")
        .with_status_code(301)
        .with_header(header("Location", location))
}

fn error(code: u16, msg: &str) -> Resp {
    Response::from_string(msg).with_status_code(code)
}

fn cookie_of(req: &Request) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv("Cookie"))
        .map(|h| h.value.as_str().to_string())
}

fn get_form(conf: &Config, cookie: Option<&str>) -> std::io::Result<Resp> {
    let options: String = conf
        .variants
        .iter()
        .map(|v| format!("<option value=\"{}\">{}</option>", v, v))
        .collect();
    let template = read_template("static/form.html")?;
    let rendered = fill_template(
        &[
            ("variants", options),
            ("version", wrench::version().to_string()),
            ("tracker", post_hog_tracker()),
        ],
        &template,
    );
    let track = get_track(cookie);
    let posthog_id = get_posthog_id_from_cookie(cookie, &format!("{}_mp", track));
    track_event(Event::GetForm { posthog_id });
    Ok(html(rendered))
}

#[derive(Clone, Debug)]
struct SimulationRequest {
    name: String,
    asm: String,
    config: String,
    comment: String,
    variant: Option<String>,
    isa: String,
}

fn parse_form(body: &[u8]) -> Result<SimulationRequest, String> {
    let fields: HashMap<String, String> = form_urlencoded::parse(body).into_owned().collect();
    let get = |k: &str| {
        fields
            .get(k)
            .cloned()
            .ok_or_else(|| format!("could not find key \"{}\"", k))
    };
    Ok(SimulationRequest {
        name: get("name")?,
        asm: get("asm")?,
        config: get("config")?,
        comment: get("comment")?,
        variant: fields.get("variant").cloned(),
        isa: get("isa")?,
    })
}

#[derive(Clone)]
struct SimulationTask {
    isa: String,
    asm_fn: String,
    conf_fn: String,
}

struct SimulationResult {
    exit_ok: bool,
    output: String,
    status_log: String,
    test_case_status: String,
    test_case: String,
    stats: String,
}

/// `show` of an exit code, as the status logs have always spelled it.
fn show_exit(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "ExitSuccess".to_string(),
        Some(c) => format!("ExitFailure {}", c),
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                format!("ExitFailure ({})", -status.signal().unwrap_or(0))
            }
            #[cfg(not(unix))]
            {
                "ExitFailure 1".to_string()
            }
        }
    }
}

fn run_wrench(conf: &Config, args: &[String]) -> (String, String, String) {
    match Command::new(&conf.wrench_path)
        .args(&conf.wrench_args)
        .args(args)
        .output()
    {
        Ok(o) => (
            show_exit(&o.status),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (
            "ExitFailure 127".to_string(),
            String::new(),
            format!("{}: {}", conf.wrench_path, e),
        ),
    }
}

fn spit_dump(conf: &Config, dir: &Path, task: &SimulationTask) -> std::io::Result<()> {
    let args: Vec<String> = ["--isa", &task.isa, &task.asm_fn, "-c", &task.conf_fn, "-S"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (_, out, err) = run_wrench(conf, &args);
    fs::write(dir.join("dump.txt"), format!("{}\n{}\n", out, err))
}

/// The UTC time as Haskell's `show` of `UTCTime` renders it.
fn show_utc_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil from days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let frac = format!("{:09}", d.subsec_nanos());
    let frac = frac.trim_end_matches('0');
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{}{} UTC",
        year,
        month,
        day,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        if frac.is_empty() { "" } else { "." },
        frac
    )
}

fn extract_stats(out: &str) -> String {
    let header = format!("# {}", execution_stats_report().name.unwrap_or_default());
    out.split("---")
        .map(str::trim)
        .find(|s| s.starts_with(&header))
        .map(|s| s[header.len()..].trim().to_string())
        .unwrap_or_default()
}

fn do_simulation(conf: &Config, task: &SimulationTask) -> SimulationResult {
    let args: Vec<String> = ["--isa", &task.isa, &task.asm_fn, "-c", &task.conf_fn, "--stats"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut cmd_parts = vec![conf.wrench_path.clone()];
    cmd_parts.extend(conf.wrench_args.iter().cloned());
    cmd_parts.extend(args.iter().cloned());
    let cmd = cmd_parts.join(" ");
    let sim_conf = fs::read_to_string(&task.conf_fn).unwrap_or_default();
    let now = show_utc_now();
    let (exit, out, err) = run_wrench(conf, &args);
    let status_log = [
        "$ date",
        &now,
        "$ wrench --version",
        wrench::version(),
        &cmd,
        &exit,
        &err,
    ]
    .join("\n");
    let n = out.chars().count();
    let output = if n > conf.log_limit {
        format!(
            "LOG TOO LONG, CROPPED\n\n{}",
            out.chars().skip(n - conf.log_limit).collect::<String>()
        )
    } else {
        out.clone()
    };
    let test_case = [
        format!("# {}", task.conf_fn),
        format!("{}===", sim_conf),
        format!("{}{}", output, err),
        "===".to_string(),
        cmd.clone(),
    ]
    .join("\n\n");
    SimulationResult {
        exit_ok: exit == "ExitSuccess",
        stats: extract_stats(&out),
        output,
        status_log,
        test_case_status: format!("{}: {}\n{}", task.conf_fn, exit, err),
        test_case,
    }
}

/// Numeric file names first, in numeric order, then the rest by name.
fn sort_files(files: &mut [String]) {
    let key = |f: &String| {
        let base = Path::new(f)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        (base.trim().parse::<i64>().ok(), base)
    };
    files.sort_by(|a, b| match (key(a), key(b)) {
        ((Some(x), _), (Some(y), _)) => x.cmp(&y),
        ((Some(_), _), _) => std::cmp::Ordering::Less,
        (_, (Some(_), _)) => std::cmp::Ordering::Greater,
        ((_, x), (_, y)) => x.cmp(&y),
    });
}

fn list_test_cases(path: &Path) -> std::io::Result<Vec<String>> {
    let mut files: Vec<String> = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".yaml"))
        .collect();
    sort_files(&mut files);
    Ok(files)
}

fn sha1_of_text(text: &str) -> String {
    let no_space = text.replace(['\t', '\n', ' '], "");
    sha1_hex(no_space.as_bytes())
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?
        .write_all(text.as_bytes())
}

fn run_job(
    conf: &Config,
    dir: &Path,
    task: SimulationTask,
    req: &SimulationRequest,
    cookie: Option<&str>,
) -> std::io::Result<()> {
    let pending = dir.join("pending");
    let start = std::time::Instant::now();
    append(&pending, "Running dump...\n")?;
    spit_dump(conf, dir, &task)?;

    append(&pending, "Running main simulation...\n")?;
    let main = do_simulation(conf, &task);
    fs::write(dir.join("status.log"), &main.status_log)?;
    fs::write(dir.join("result.log"), &main.output)?;
    append(&pending, "Main simulation done.\n")?;

    let mut checks = Vec::new();
    if let Some(variant) = &req.variant {
        let vdir = Path::new(&conf.variants_path).join(variant);
        let cases = list_test_cases(&vdir)?;
        let total = cases.len();
        append(&pending, &format!("Running {} variant test cases...\n", total))?;
        for (i, case) in cases.iter().enumerate() {
            append(&pending, &format!("Test case {}/{}...\n", i + 1, total))?;
            let t = SimulationTask {
                conf_fn: vdir.join(case).to_string_lossy().into_owned(),
                ..task.clone()
            };
            checks.push(do_simulation(conf, &t));
        }
    }
    let statuses: String = checks.iter().map(|c| c.test_case_status.as_str()).collect();
    fs::write(dir.join("test_cases_status.log"), statuses)?;
    let fails: Vec<&SimulationResult> = checks.iter().filter(|c| !c.exit_ok).collect();
    fs::write(
        dir.join("test_cases_result.log"),
        fails.first().map_or("", |f| f.test_case.as_str()),
    )?;
    // Execution stats for every variant case, headed by the case's status line.
    let stats_log = checks
        .iter()
        .map(|c| format!("# {}\n{}", c.test_case_status.lines().next().unwrap_or(""), c.stats))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    fs::write(dir.join("stats.log"), stats_log)?;

    let secs = start.elapsed().as_secs();
    let track = get_track(cookie);
    let posthog_id = get_posthog_id_from_cookie(cookie, &format!("{}_mp", track));
    track_event(Event::Simulation {
        guid: dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        name: req.name.clone(),
        isa: req.isa.clone(),
        variant: req.variant.clone(),
        asm_sha1: sha1_of_text(&req.asm),
        yaml_sha1: sha1_of_text(&req.config),
        win_count: checks.len() - fails.len(),
        fail_count: fails.len(),
        posthog_id,
        success: main.exit_ok,
        duration: secs,
        variant_success: (!checks.is_empty()).then_some(fails.is_empty()),
    });
    append(&pending, &format!("All done in {}s.\n", secs))?;
    fs::rename(&pending, dir.join("pending.done"))
}

fn submit_form(conf: Arc<Config>, cookie: Option<String>, body: &[u8]) -> std::io::Result<Resp> {
    let req = match parse_form(body) {
        Ok(r) => r,
        Err(e) => return Ok(error(400, &e)),
    };
    let guid = Uuid::new_v4();
    let dir = PathBuf::from(&conf.storage_path).join(guid.to_string());
    fs::create_dir_all(&dir)?;
    for (file, content) in [
        ("source.s", req.asm.as_str()),
        ("config.yaml", req.config.as_str()),
        ("name.txt", req.name.as_str()),
        ("comment.txt", req.comment.as_str()),
        ("variant.txt", req.variant.as_deref().unwrap_or("-")),
        ("isa.txt", req.isa.as_str()),
        ("wrench-version.txt", wrench::version()),
    ] {
        fs::write(dir.join(file), content)?;
    }
    let task = SimulationTask {
        isa: req.isa.clone(),
        asm_fn: dir.join("source.s").to_string_lossy().into_owned(),
        conf_fn: dir.join("config.yaml").to_string_lossy().into_owned(),
    };
    fs::write(dir.join("pending"), "Starting simulation...\n")?;
    let job_dir = dir.clone();
    let job_cookie = cookie.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_job(&conf, &job_dir, task, &req, job_cookie.as_deref()) {
            let _ = append(&job_dir.join("pending"), &format!("ERROR: {}\n", e));
        }
    });
    let track = get_track(cookie.as_deref());
    Ok(redirect(&format!("/report/{}", guid)).with_header(header("Set-Cookie", &track_cookie(&track))))
}

fn read_text(dir: &Path, file: &str) -> std::io::Result<String> {
    fs::read(dir.join(file)).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn version_warning(report_version: &str) -> String {
    if report_version == wrench::version() {
        String::new()
    } else {
        format!(
            " <span class=\"text-[var(--c-orange)]\">[WARNING: current wrench version is {}]</span>",
            escape_html(wrench::version())
        )
    }
}

fn get_report(conf: &Config, cookie: Option<&str>, guid: &str) -> std::io::Result<Resp> {
    let Ok(guid) = Uuid::parse_str(guid) else {
        return Ok(error(400, "Error parsing path parameter guid"));
    };
    let storage = PathBuf::from(&conf.storage_path).join(guid.to_string());
    let dir = if storage.is_dir() {
        storage
    } else {
        PathBuf::from(&conf.examples_path).join(guid.to_string())
    };
    let track = get_track(cookie);
    if dir.join("pending").exists() {
        let log = read_text(&dir, "pending")?;
        let template = read_template("static/inprogress.html")?;
        let rendered = fill_template(
            &[("log", escape_html(&log)), ("tracker", post_hog_tracker())],
            &template,
        );
        return Ok(html(rendered).with_header(header("Set-Cookie", &track_cookie(&track))));
    }
    let name = read_text(&dir, "name.txt")?;
    let variant = read_text(&dir, "variant.txt")?;
    let comment = read_text(&dir, "comment.txt")?;
    let asm = read_text(&dir, "source.s")?;
    let config = read_text(&dir, "config.yaml")?;
    let log = read_text(&dir, "result.log")?;
    let status = read_text(&dir, "status.log")?;
    let test_case_status = read_text(&dir, "test_cases_status.log")?;
    let test_case_result = read_text(&dir, "test_cases_result.log")?;
    let report_version = read_text(&dir, "wrench-version.txt").unwrap_or_else(|_| "< 0.2.11".to_string());
    let dump = read_text(&dir, "dump.txt").unwrap_or_else(|_| "DUMP NOT AVAILABLE".to_string());
    let stats = read_text(&dir, "stats.log").unwrap_or_default();
    let isa = read_text(&dir, "isa.txt").ok().and_then(|s| Isa::from_name(s.trim()));
    let template = read_template("static/result.html")?;
    let rendered = fill_template(
        &[
            ("tracker", post_hog_tracker()),
            ("name", escape_html(&name)),
            ("variant", escape_html(&variant)),
            ("comment", escape_html(&comment)),
            ("status", escape_html(&status)),
            ("test_cases_status", escape_html(&test_case_status)),
            ("report_wrench_version", escape_html(&report_version)),
            ("version_warning", version_warning(&report_version)),
            ("assembler_code", format_asm(isa, &asm)),
            ("yaml_content", format_code_with_line_numbers(&config)),
            ("result", format_report(isa, &log)),
            ("test_cases_result", format_report(isa, &test_case_result)),
            ("execution_stats", format_report(isa, &stats)),
            ("dump", format_report(isa, &dump)),
        ],
        &template,
    );
    let posthog_id = get_posthog_id_from_cookie(cookie, &format!("{}_mp", track));
    track_event(Event::ReportView {
        guid: guid.to_string(),
        name,
        posthog_id,
        wrench_version: report_version,
    });
    Ok(html(rendered).with_header(header("Set-Cookie", &track_cookie(&track))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExampleEntry {
    guid: String,
    isa: String,
    title: String,
    description: String,
    ok: bool,
}

fn read_examples_index(path: &str) -> Vec<ExampleEntry> {
    let index = Path::new(path).join("index.json");
    let Ok(raw) = fs::read(&index) else {
        return Vec::new();
    };
    serde_json::from_slice(&raw).unwrap_or_else(|e| {
        println!("Failed to parse {}: {}", index.display(), e);
        Vec::new()
    })
}

fn render_item(e: &ExampleEntry) -> String {
    let (class, label) = if e.ok {
        ("text-[var(--c-green)]", "ok")
    } else {
        ("text-[var(--c-orange)]", "fail")
    };
    let desc = if e.description.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"w-full pl-[3ch] text-[var(--c-grey)] text-sm\">{}</div>",
            escape_html(&e.description)
        )
    };
    format!(
        "<li class=\"flex flex-wrap items-baseline gap-x-2\"><span class=\"{}\">[{}]</span><a href=\"/report/{}\" class=\"hover:bg-[var(--c-fuschia)] pt-[0.2ch] pb-[0.2ch] text-[var(--c-fuschia)] hover:text-[var(--c-black)] cursor-pointer\">[{}]</a>{}</li>",
        class,
        label,
        escape_html(&e.guid),
        escape_html(&e.title),
        desc
    )
}

fn render_examples(mut entries: Vec<ExampleEntry>) -> String {
    if entries.is_empty() {
        return "<p class=\"text-[var(--c-grey)]\">No examples available.</p>".to_string();
    }
    entries.sort_by(|a, b| (&a.isa, &a.title).cmp(&(&b.isa, &b.title)));
    let mut out = String::new();
    for group in entries.chunk_by(|a, b| a.isa == b.isa) {
        let isa = escape_html(&group[0].isa);
        out.push_str(&format!(
            "<div id=\"{isa}\" class=\"mb-8 scroll-mt-4\"><h2 class=\"mb-3 pb-1 border-b border-zinc-700 text-[var(--c-grey)] text-xl\"><a href=\"#{isa}\" class=\"hover:bg-[var(--c-grey)] hover:text-[var(--c-black)] cursor-pointer\">/* {isa} */</a></h2><ul class=\"space-y-1\">{}</ul></div>",
            group.iter().map(render_item).collect::<String>()
        ));
    }
    out
}

fn get_examples(conf: &Config) -> std::io::Result<Resp> {
    let template = read_template("static/examples.html")?;
    let rendered = fill_template(
        &[
            ("examples", render_examples(read_examples_index(&conf.examples_path))),
            ("tracker", post_hog_tracker()),
            ("version", wrench::version().to_string()),
        ],
        &template,
    );
    Ok(html(rendered))
}

fn get_asset(path: &str) -> Resp {
    let name = path.split('?').next().unwrap_or("");
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return error(404, "Not Found");
    }
    let content_type = match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("html") => "text/html",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    match fs::read(Path::new(ASSETS_DIR).join(name)) {
        Ok(bytes) => Response::from_data(bytes).with_header(header("Content-Type", content_type)),
        Err(_) => error(404, "Not Found"),
    }
}

fn handle(conf: Arc<Config>, mut req: Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = req.method().clone();
    let cookie = cookie_of(&req);
    let resp: std::io::Result<Resp> = match (&method, path.as_str()) {
        (Method::Get, "/submit-form") => get_form(&conf, cookie.as_deref()),
        (Method::Post, "/submit") => {
            let mut body = Vec::new();
            match req.as_reader().read_to_end(&mut body) {
                Ok(_) => submit_form(conf.clone(), cookie, &body),
                Err(e) => Err(e),
            }
        }
        (Method::Get, "/examples") => get_examples(&conf),
        (Method::Get, "/") => Ok(redirect("/submit-form")),
        (Method::Get, p) if p.starts_with("/report/") => get_report(&conf, cookie.as_deref(), &p["/report/".len()..]),
        (Method::Get, p) if p.starts_with("/assets/") => Ok(get_asset(&url["/assets/".len()..])),
        _ => Ok(error(404, "Not Found")),
    };
    let resp = resp.unwrap_or_else(|e| {
        eprintln!("{} {}: {}", method, path, e);
        error(500, "Something went wrong")
    });
    println!("{} {} {}", method, url, resp.status_code().0);
    let _ = req.respond(resp);
}

fn main() {
    let conf = Arc::new(init_config());
    println!("{:?}", conf);
    println!("Starting server on port {}", conf.port);
    let server = Server::http(("0.0.0.0", conf.port)).expect("cannot bind the port");
    for req in server.incoming_requests() {
        let conf = conf.clone();
        std::thread::spawn(move || handle(conf, req));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_fills_once_and_keeps_unknown() {
        let vars = [("a", "{{b}}".to_string()), ("b", "x".to_string())];
        assert_eq!(fill_template(&vars, "{{a}} {{b}} {{c}} {{"), "{{b}} x {{c}} {{");
    }

    #[test]
    fn numeric_files_sort_first() {
        let mut fs = vec![
            "b.yaml".to_string(),
            "10.yaml".to_string(),
            "2.yaml".to_string(),
            "a.yaml".to_string(),
        ];
        sort_files(&mut fs);
        assert_eq!(fs, vec!["2.yaml", "10.yaml", "a.yaml", "b.yaml"]);
    }

    #[test]
    fn stats_are_cut_from_output() {
        let out = "---\n# Log\nx\n---\n# Overview\n## Execution statistics\nfoo\n";
        assert_eq!(extract_stats(out), "## Execution statistics\nfoo");
    }
}
