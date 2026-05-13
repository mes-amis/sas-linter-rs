use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use sas_linter::{rules, Config, Linter};

#[derive(Parser, Debug)]
#[command(
    name = "sas-lint",
    version,
    about = "Configurable lint engine for SAS source files."
)]
struct Cli {
    /// YAML config file. Missing file → run with built-in defaults.
    #[arg(long, default_value = "config/lint.yaml")]
    config: PathBuf,

    /// Comma-separated list of rule ids to run with default options.
    /// Overrides --config.
    #[arg(long, value_delimiter = ',')]
    rules: Option<Vec<String>>,

    /// Reformat file(s) in place. Not yet implemented — accepted for
    /// CLI compatibility with the Ruby gem; will be wired up after the
    /// formatter port lands.
    #[arg(long)]
    format: bool,

    /// Suppress autofix even if the config sets `autofix: true` for some
    /// rule. Findings are still reported but no file is rewritten.
    #[arg(long = "no-autofix")]
    no_autofix: bool,

    /// Print every registered rule and exit.
    #[arg(long)]
    list_rules: bool,

    /// Files to lint.
    files: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("sas-lint: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    if cli.list_rules {
        for meta in rules::all_metas() {
            let mark = if meta.supports_autofix {
                "  [autofix]"
            } else {
                ""
            };
            println!("{:<40}  {}{}", meta.id, meta.description, mark);
        }
        return Ok(ExitCode::from(0));
    }

    if cli.files.is_empty() {
        eprintln!("Usage: sas-lint FILE [FILE ...] [options]");
        return Ok(ExitCode::from(2));
    }

    if cli.format {
        eprintln!("sas-lint: --format is not yet implemented in the Rust port");
        return Ok(ExitCode::from(2));
    }

    let linter = if let Some(ids) = &cli.rules {
        Linter::from_ids(ids)?
    } else {
        let mut cfg = Config::load(&cli.config)?;
        if cli.no_autofix {
            cfg.strip_autofix();
        }
        Linter::from_config(&cfg)?
    };

    let mut exit_code: u8 = 0;
    for path in &cli.files {
        if !path.is_file() {
            eprintln!("sas-lint: {}: not a regular file", path.display());
            exit_code = exit_code.max(2);
            continue;
        }
        let findings = linter.lint_file(path)?;
        if findings.is_empty() {
            continue;
        }
        exit_code = exit_code.max(1);
        for f in findings {
            println!("{}", f);
        }
    }

    Ok(ExitCode::from(exit_code))
}
