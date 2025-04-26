use serde::Deserialize;
use std::error::Error;
use std::fmt;
use std::io;
use std::process::{self, Command};
use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum GhError {
    Io(io::Error),
    Failed(process::Output),
    Unicode(FromUtf8Error),
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Unicode(e) => e.fmt(f),
            Self::Failed(output) => {
                f.write_fmt(format_args!("gh {}\n", output.status))?;
                f.write_str(&String::from_utf8_lossy(&output.stderr))
            }
        }
    }
}

impl Error for GhError {}

#[derive(Default, Debug)]
pub struct GithubApi {
    api: String,
    headers: Vec<String>,
    fields: Vec<String>,
    all_pages: bool,
}
impl GithubApi {
    pub fn new(api: &str) -> Self {
        Self {
            api: api.into(),
            ..Self::default()
        }
    }

    pub fn field(&mut self, field: &str) -> &mut Self {
        self.fields.push(field.into());
        self
    }

    pub fn fields<'a, I: IntoIterator<Item = &'a str>>(&mut self, fields: I) -> &mut Self {
        for field in fields {
            self.field(field);
        }
        self
    }

    pub fn all_pages(&mut self) -> &mut Self {
        self.all_pages = true;
        self
    }

    pub fn run(&mut self) -> Result<String, GhError> {
        self.raw_output()
            .and_then(|output| String::from_utf8(output).map_err(GhError::Unicode))
    }

    pub fn raw_output(&mut self) -> Result<Vec<u8>, GhError> {
        let mut cmd = Command::new("gh");
        cmd.args(["api", &self.api]);
        cmd.args(["--method", "GET"]);

        let default_headers = [
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ];
        for header in self
            .headers
            .iter()
            .map(AsRef::as_ref)
            .chain(default_headers)
        {
            cmd.args(["-H", header]);
        }
        for field in &self.fields {
            cmd.args(["-f", field]);
        }
        if self.all_pages {
            cmd.args(["--paginate", "--slurp"]);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    Ok(output.stdout)
                } else {
                    Err(GhError::Failed(output))
                }
            }
            Err(e) => Err(GhError::Io(e)),
        }
    }
}

#[derive(Deserialize)]
pub struct WorkflowRuns {
    pub workflow_runs: Vec<WorkflowRun>,
}

#[allow(unused)]
#[derive(Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub display_title: String,
    pub run_number: u64,
    pub status: String,
    pub conclusion: Option<Conclusion>,
    pub check_suite_id: u64,
    pub url: String,
    pub html_url: String,
    pub run_attempt: u64,
}

#[derive(Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Conclusion {
    Failure,
    Success,
    Cancelled,
    Skipped,
}

#[derive(Deserialize)]
pub struct Jobs {
    pub jobs: Vec<Job>,
}

#[derive(Deserialize, Debug)]
pub struct Job {
    pub id: u64,
    pub html_url: String,
    pub conclusion: Conclusion,
    pub started_at: String,
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Deserialize, Debug)]
pub struct Step {
    pub name: String,
    pub conclusion: Conclusion,
    pub number: u64,
}
