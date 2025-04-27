//!
use clap::Parser;

mod github;
mod strip_ansi;

use core::time::Duration;
use jiff::Timestamp;
use std::{
    fs,
    process::{Command, ExitCode},
};
use strip_ansi::AnsiMode;

use github::{Conclusion, GithubApi, WorkflowRuns};

const FULL_LOGS: bool = cfg!(feature = "download_full_logs");

// TODO: proper error handling
macro_rules! fail {
    ($($arg:tt)*) => {{
        eprint!("{}:{}:{} ", file!(), line!(), column!());
        eprintln!($($arg)*);
        return ExitCode::FAILURE;
    }};
}

struct FailedWorkflowRun {
    id: u64,
    title: String,
}

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAYS: u64 = 24 * HOUR;

fn main() -> ExitCode {
    let cli = Cli::parse();
    // FIXME: proper arg validation
    let start = cli
        .start_date
        .and_then(|t| Timestamp::strptime("%Y-%m-%d%z", format!("{t}+0000")).ok())
        .unwrap_or(Timestamp::now() - Duration::from_secs(30 * DAYS));
    let end = cli
        .end_date
        .and_then(|t| Timestamp::strptime("%Y-%m-%d%z", format!("{t}+0000")).ok())
        .unwrap_or(Timestamp::now());
    let start = start.strftime("%Y-%m-%d");
    let end = end.strftime("%Y-%m-%d");
    let range = format!("{start}..{end}");

    let run_dir = "cache/runs";
    let todays_cache = format!("{run_dir}/{range}.json");
    if let Err(e) = fs::create_dir_all(run_dir) {
        fail!("filesystem error: {e}\n in path {run_dir}");
    }

    // FIXME: improve caching
    let runs = if !fs::exists(&todays_cache).unwrap_or(false) {
        let result = GithubApi::new("repos/rust-lang-ci/rust/actions/runs")
            .fields([
                "status=completed",
                "branch=auto",
                "per_page=100",
                &format!("created={range}"),
            ])
            .all_pages()
            .run();

        let runs = match result {
            Ok(output) => output,
            Err(e) => fail!("github error: {e}"),
        };

        if let Err(e) = fs::write(&todays_cache, &runs) {
            fail!("filesystem error: {e}\n in path {todays_cache}");
        }
        runs
    } else {
        match fs::read_to_string(&todays_cache) {
            Err(e) => fail!("filesystem error: {e}\n in path {todays_cache}"),
            Ok(runs) => runs,
        }
    };

    let runs: Vec<WorkflowRuns> = match serde_json::from_str(&runs) {
        Ok(runs) => runs,
        Err(e) => fail!("serde error: {e}"),
    };

    let mut fail_count = 0;
    let mut success_count = 0;
    let mut cancelled_count = 0;
    let mut failures = Vec::new();
    for runs in runs {
        for runs in runs.workflow_runs {
            if let Some(conclusion) = runs.conclusion {
                if conclusion == Conclusion::Failure {
                    failures.push(FailedWorkflowRun {
                        id: runs.id,
                        title: runs.display_title,
                    });
                    fail_count += 1;
                }
                if conclusion == Conclusion::Success {
                    success_count += 1;
                } else if conclusion == Conclusion::Cancelled {
                    cancelled_count += 1;
                }
            }
        }
    }

    let jobs_dir = "cache/jobs";
    if let Err(e) = fs::create_dir_all(jobs_dir) {
        fail!("filesystem error: {e}\n in path {jobs_dir}");
    }
    let run_logs_dir = "cache/logs/runs";
    if let Err(e) = fs::create_dir_all(run_logs_dir) {
        fail!("filesystem error: {e}\n in path {run_logs_dir}");
    }
    let jobs_logs_dir = "cache/logs/jobs";
    if let Err(e) = fs::create_dir_all(jobs_logs_dir) {
        fail!("filesystem error: {e}\n in path {jobs_logs_dir}");
    }

    let total = fail_count + success_count;
    println!("Failed workflow runs ({fail_count}/{total}, +{cancelled_count} cancelled):");
    let mut fails = Fails {
        start: start.to_string(),
        end: end.to_string(),
        success: success_count,
        fail: fail_count,
        cancelled: cancelled_count,
        fails: Vec::new(),
    };
    for FailedWorkflowRun { id, title } in failures {
        println!("{id}: {title}");

        let job_path = format!("{jobs_dir}/{id}.json");
        let jobs = if !fs::exists(&job_path).unwrap_or(false) {
            let result = GithubApi::new(&format!("repos/rust-lang-ci/rust/actions/runs/{id}/jobs"))
                .field("per_page=100")
                .run();

            let job = match result {
                Ok(output) => output,
                Err(e) => fail!("github error: {e}"),
            };

            if let Err(e) = fs::write(&job_path, &job) {
                fail!("filesystem error: {e}\n in path {job_path}");
            }
            job
        } else {
            match fs::read_to_string(&job_path) {
                Err(e) => fail!("filesystem error: {e}\n in path {job_path}"),
                Ok(jobs) => jobs,
            }
        };
        let jobs: github::Jobs = match serde_json::from_str(&jobs) {
            Ok(jobs) => jobs,
            Err(e) => fail!("serde error: {e}"),
        };

        if FULL_LOGS {
            // Download the full logs so we can select only the step that failed.
            // This will produce very large zip files so not recommended.
            let run_logs_path = format!("{run_logs_dir}/{id}.zip");
            if !fs::exists(&run_logs_path).unwrap_or(false) {
                let result =
                    GithubApi::new(&format!("repos/rust-lang-ci/rust/actions/runs/{id}/logs"))
                        .raw_output();

                let logs = match result {
                    Ok(output) => output,
                    Err(e) => fail!("github error: {e}"),
                };

                if let Err(e) = fs::write(&run_logs_path, &logs) {
                    fail!("filesystem error: {e}\n in path {run_logs_path}");
                }
            }

            // extract the logs
            let extract_dir = &run_logs_path[..run_logs_path.len() - 4];
            if !fs::exists(extract_dir).unwrap_or(false) {
                if let Err(e) = fs::create_dir_all(extract_dir) {
                    fail!("filesystem error: {e}\n in path {jobs_logs_dir}");
                }
                let mut to_extract = String::new();
                'jobs: for job in jobs.jobs {
                    if job.conclusion == Conclusion::Failure {
                        for step in job.steps {
                            if step.conclusion == Conclusion::Failure {
                                let job_name = &job.name;
                                let step_number = step.number;
                                let step_name = step.name;
                                to_extract = format!("{job_name}/{step_number}_{step_name}.txt");
                                break 'jobs;
                            }
                        }
                    }
                }
                if to_extract.is_empty() {
                    let _ = fs::remove_dir_all(extract_dir);
                    fail!("no failed logs for workflow {id}");
                }
                match Command::new("tar")
                    .args(["-xf", &run_logs_path, "-C", extract_dir])
                    .arg(&to_extract)
                    .status()
                {
                    Ok(status) if !status.success() => {
                        let _ = fs::remove_dir_all(extract_dir);
                        fail!("tar failed to extract the archive at {run_logs_path}")
                    }
                    Err(e) => {
                        let _ = fs::remove_dir_all(extract_dir);
                        fail!("tar failed to run: {e}")
                    }
                    Ok(_) => {}
                }
            }
            todo!("either finish writing this or delete it");
        } else {
            // Download only the failed logs.
            // Smaller but not separated by step.
            // Should be fine though, trimming it seems to work.
            for job in jobs.jobs {
                // Skip success and bors.
                if job.conclusion != Conclusion::Failure || job.name == "bors build finished" {
                    continue;
                }
                let job_id = job.id;
                let job_log_path = format!("{jobs_logs_dir}/{job_id}.txt");
                let mut log = if !fs::exists(&job_log_path).unwrap_or(false) {
                    let result = GithubApi::new(&format!(
                        "repos/rust-lang-ci/rust/actions/jobs/{job_id}/logs"
                    ))
                    .run();
                    let log = match result {
                        Ok(output) => output,
                        Err(e) => fail!("github error: {e}"),
                    };

                    if let Err(e) = fs::write(&job_log_path, &log) {
                        fail!("filesystem error: {e}\n in path {job_log_path}");
                    }
                    log
                } else {
                    match fs::read_to_string(&job_log_path) {
                        Err(e) => fail!("filesystem error: {e}\n in path {job_log_path}"),
                        Ok(log) => log,
                    }
                };

                trim_log(&mut log);
                let short_log = short_log(&log);
                // Parse the PR id from the title
                let pr_id: u64 = if let Some(text_id) = title
                    .strip_prefix("Auto merge of #")
                    .and_then(|s| s.split_once(" ").map(|s| s.0))
                {
                    match text_id.parse() {
                        Ok(id) => id,
                        Err(e) => fail!("PR id not found: {e}"),
                    }
                } else {
                    fail!("PR id not found");
                };
                let error_line = error_line(&short_log).map(String::from);
                fails.fails.push(Fail {
                    title: title.clone(),
                    job_name: job.name,
                    job_id: job.id,
                    url: job.html_url,
                    time: job.started_at,
                    //log,
                    short_log,
                    error_line,
                    pr_id,
                });
            }
        }
    }

    let report_dir = format!("report/{start}..{end}/");
    if let Err(e) = fs::create_dir_all(&report_dir) {
        fail!("filesystem error: {e}\n in path {report_dir}");
    }

    let json_path = report_dir.clone() + "report.json";
    match serde_json::to_string_pretty(&fails) {
        Ok(s) => {
            if let Err(e) = fs::write(&json_path, &s) {
                fail!("filesystem error: {e}\n in path {json_path}");
            }
        }
        Err(e) => fail!("serialization failed: {e}"),
    }
    let html_path = report_dir + "report.html";
    let html = make_html(&fails);
    if let Err(e) = fs::write(&html_path, &html) {
        fail!("filesystem error: {e}\n in path {html_path}");
    }

    ExitCode::SUCCESS
}

fn trim_log(log: &mut String) {
    // remove time lines
    let mut strip_log = String::new();
    for line in log.lines() {
        let line = if let Some((head, tail)) = line.split_once(" ") {
            // FIXME: Exactly specify the expected format
            if head.parse::<Timestamp>().is_ok() {
                tail
            } else {
                line
            }
        } else {
            line
        };
        strip_log.push_str(line);
        strip_log.push('\n');
    }
    *log = strip_log;

    // remove ansi escapes
    let mut strip_log = Vec::new();
    let mut mode = AnsiMode::Text;
    for &b in log.as_bytes() {
        if mode.update(b).is_text() {
            strip_log.push(b);
        }
    }
    *log = String::from_utf8(strip_log).unwrap();

    // remove the cleanup step from logs
    if let Some(pos) = log.rfind("\nPost job cleanup.\n") {
        log.truncate(pos + 1);
    }

    // Get only the last group.
    if let Some(pos) = log.rfind("\n##[group") {
        log.replace_range(..pos + 1, "");
    }
}

fn short_log(log: &str) -> String {
    let group = if log.starts_with("##[group") {
        log.lines().next()
    } else {
        None
    };
    if let Some(pos) = log.find("\nfailures:\n") {
        log[pos..].into()
    } else if let Some(pos) = log.find("\n##[error]The runner has received a shutdown signal.") {
        // No point printing the full logs if the run was essentially cancelled by outside forces.
        log[pos..].into()
    } else if group.is_some_and(|g| g.starts_with("##[group]Building LLVM for ")) {
        let mut short = group.unwrap().to_string();
        if let Some(pos) = log.find("\nFAILED: ") {
            short.push_str(&log[pos..]);
            short
        } else {
            // we couldn't find a failure message but we truncate the output anyway
            // because otherwise it can be gigantic.
            short.push('\n');
            let log = tail_lines(&log[short.len()..], 50);
            short.push_str(log);
            short
        }
    } else {
        // limit the logs to some reasonable number of lines.
        let short_log = tail_lines(log, 500);
        if short_log.len() != log.len() {
            if let Some(group) = group {
                String::from(group) + short_log
            } else {
                short_log.into()
            }
        } else {
            log.into()
        }
    }
}

fn tail_lines(log: &str, lines: usize) -> &str {
    let mut pos = log.len();
    for _ in 0..lines {
        if pos == 0 {
            break;
        } else if let Some(new_pos) = log[..pos - 1].rfind("\n") {
            pos = new_pos + 1
        } else {
            pos = 0;
        }
    }
    &log[pos..]
}

fn error_line(log: &str) -> Option<&str> {
    for line in log.lines() {
        if line == "##[error]Process completed with exit code 1." {
            // This is the "something went wrong" of errors.
            continue;
        } else if line.starts_with("error: ")
            || line.starts_with("error[")
            || line.starts_with("rustc exited with signal:")
            || line.starts_with("##[error]")
            || line.starts_with("TypeError:")
            || line.starts_with("dyld[")
        {
            if !line.starts_with("error: test failed, to rerun") {
                return Some(line);
            }
        }
    }

    // We didn't find any errors. Let's look for some lesser candidates.
    let mut report_next = false;
    let mut previous = "";
    for line in log.lines() {
        if report_next {
            // Fixme: report both lines.
            if line.trim().is_empty()
                || line == "explicit panic"
                || line == "assertion `left == right` failed"
            {
                return Some(previous);
            } else {
                return Some(line);
            }
        } else if line.starts_with("ERROR: ")
            || line.starts_with("error in revision ")
            || line.starts_with("fatal: ")
        {
            return Some(line);
        } else if line.starts_with("thread '")
            && line.contains("' panicked at ")
            && line.ends_with(":")
        {
            previous = line;
            report_next = true;
        }
    }

    None
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Fails {
    start: String,
    end: String,
    success: u64,
    fail: u64,
    cancelled: u64,
    fails: Vec<Fail>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Fail {
    title: String,
    time: String,
    job_name: String,
    job_id: u64,
    url: String,
    //log: String,
    short_log: String,
    error_line: Option<String>,
    pr_id: u64,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// The start date, in YY-mm-dd format. E.g. 2025-04-01.
    start_date: Option<String>,
    /// The end date, in YY-mm-dd format. E.g. 2025-05-01.
    end_date: Option<String>,
}

// FIXME: do this properly
fn make_html(fails: &Fails) -> String {
    let Fails {
        start,
        end,
        success,
        fail,
        cancelled,
        fails,
    } = fails;
    let total = success + fail;
    let percent = (fail * 100) / total;
    let mut html = String::new();
    html.push_str(
        r#"<!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>Rust CI report</title>
            <link rel="stylesheet" href="styles.css">
        </head>
        "#,
    );
    html.push_str(&format!(
        "
        <h1>Rustc CI failures {start} to {end}</h1>
        <article id=\"stats\">
            <h2>Stats</h2>
            <p>fails: {fail}/{total} ({percent}%)</p>
            <p>cancelled: {cancelled}</p>
        </article>
        "
    ));
    // TODO: create a summary table.
    let mut summary = String::from(
        "<section id = \"summary\">
        <h2>Summary</h2>
        <table><thead><tr><th>Time (UTC)</th><th>PR</th><th>Job Name</th><th>Short Log</th><th>Error Message</th></tr></thead>
        <tbody>
        ",
    );
    let mut logs = String::from("<section id = \"logs\"><h2>Short logs</h2>");
    for fail in fails {
        let Fail {
            title,
            time,
            job_name,
            job_id,
            url,
            short_log,
            error_line,
            pr_id,
        } = fail;
        let mut short_log = short_log.replace("&", "&amp;");
        short_log = short_log.replace("<", "&lt;");
        short_log = short_log.replace(">", "&gt;");
        let error_line = error_line.as_deref().unwrap_or("");
        summary.push_str(&format!(
            "
            <tr>
            <td>{time}</td>
            <td><a href=\"https://github.com/rust-lang/rust/pull/{pr_id}\">#{pr_id}</a></td>
            <td>{job_name}</td>
            <td><a href=\"#job-{job_id}\">log</a></td>
            <td class=\"error_msg\"><pre><code>{error_line}</code></pre></td>
            </tr>
            ",
        ));
        logs.push_str(&format!(
            "
            <article id=\"job-{job_id}\" class=\"failure\">
                <h3><a href=\"{url}\">{title}</a></h3>
                <p>{job_name}</p>
                <p>{time}</p>
                <pre class=\"log\"><code>{short_log}</code></pre>
            </article> 
            "
        ));
    }
    summary.push_str("</tbody></table></section>");
    logs.push_str("</section>");
    html.push_str(&summary);
    html.push_str(&logs);

    html.push_str(
        r#"
        <script src="script.js"></script>
        <style>
        .log { overflow: auto; border: 1px solid black; padding: 1em; background-color: #eee; }
        table { border-collapse: collapse; }
        thead tr { border-bottom: 2px solid white; }
        th { position: sticky; top: 0; background-color: white; }
        td { border: 2px solid white; padding: 5px; }
        tr:nth-child(even) { background: #eee; }
        .error_msg { font-size: 12px; }
        .error_msg pre { white-space: pre-wrap; word-wrap: break-word; }
        </style>
        </html>
        "#,
    );
    html
}
