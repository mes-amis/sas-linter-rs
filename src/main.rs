use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sas_linter::encoding::read_source;
use sas_linter::value_specs::{SpecSource, ValueSpecs};
use sas_linter::{rules, synthesize, Config, Formatter, Linter};

#[derive(Parser, Debug)]
#[command(
    name = "sas-lint",
    version,
    about = "Configurable lint engine for SAS source files.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// YAML config file. Missing file → run with built-in defaults.
    #[arg(long, default_value = "config/lint.yaml")]
    config: PathBuf,

    /// Comma-separated list of rule ids to run with default options.
    /// Overrides --config.
    #[arg(long, value_delimiter = ',')]
    rules: Option<Vec<String>>,

    /// Reformat file(s) in place (keyword casing, operator spacing,
    /// indent, comment/assignment alignment), then apply autofix-enabled
    /// rules.
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

#[derive(Subcommand, Debug)]
enum Command {
    /// Synthesize input testdata (CSV) from each SAS source's logic graph.
    ///
    /// Walks the source's arrays, assignments, and `if`-head predicates,
    /// builds per-input value sets (seeded from the
    /// `variable_value_out_of_known_range` metadata CSVs in --config, when
    /// configured), and writes --size sampled rows per source. Each row
    /// is drawn from one of five strategies: baseline (everything at
    /// default), single (satisfy one predicate), combo (satisfy 2–6 at
    /// once), perturbation (baseline plus a few random cell flips), and
    /// chaos (uniform sampling). Deterministic for a given --seed.
    Synthesize(SynthesizeArgs),
}

#[derive(clap::Args, Debug)]
struct SynthesizeArgs {
    /// Output directory (writes <dir>/<source stem>.csv per source), or a
    /// `.csv` file path when synthesizing a single source.
    #[arg(short = 'o', long = "output")]
    output: PathBuf,

    /// Number of rows to sample per source.
    #[arg(long, default_value_t = synthesize::DEFAULT_SIZE as u64,
          value_parser = clap::value_parser!(u64).range(1..))]
    size: u64,

    /// RNG seed.
    #[arg(long, default_value_t = synthesize::DEFAULT_SEED)]
    seed: u64,

    /// YAML config whose `variable_value_out_of_known_range` options
    /// locate the acceptable-values metadata. Missing file → no metadata.
    #[arg(long, default_value = "config/lint.yaml")]
    config: PathBuf,

    /// SAS source files.
    #[arg(required = true)]
    sources: Vec<PathBuf>,
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

    if let Some(Command::Synthesize(args)) = &cli.command {
        return run_synthesize(args);
    }

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
        let cfg = Config::load(&cli.config)?;
        let formatter = Formatter::from_config(&cfg)?;
        let linter = if let Some(ids) = &cli.rules {
            Linter::from_ids(ids)?
        } else {
            Linter::from_config(&cfg)?
        };

        let mut exit_code: u8 = 0;
        for path in &cli.files {
            if !path.is_file() {
                eprintln!("sas-lint: {}: not a regular file", path.display());
                exit_code = exit_code.max(2);
                continue;
            }
            linter.format_file(path, &formatter)?;
        }
        return Ok(ExitCode::from(exit_code));
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

fn run_synthesize(args: &SynthesizeArgs) -> Result<ExitCode> {
    let single_file = args
        .output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if single_file && args.sources.len() > 1 {
        eprintln!(
            "sas-lint synthesize: {} is a file but {} sources were given; pass a directory to -o",
            args.output.display(),
            args.sources.len()
        );
        return Ok(ExitCode::from(2));
    }

    let cfg = Config::load(&args.config)?;
    let specs = match cfg.rules.get("variable_value_out_of_known_range") {
        Some(opts) => SpecSource::from_opts(opts)?.load(),
        None => ValueSpecs::default(),
    };

    let out_dir = if single_file {
        args.output.parent().unwrap_or(Path::new("")).to_path_buf()
    } else {
        args.output.clone()
    };
    if !out_dir.as_os_str().is_empty() {
        fs::create_dir_all(&out_dir)
            .with_context(|| format!("creating output directory {}", out_dir.display()))?;
    }

    let mut exit_code: u8 = 0;
    for source in &args.sources {
        if !source.is_file() {
            eprintln!("sas-lint synthesize: {}: no such file", source.display());
            exit_code = 2;
            continue;
        }
        let text = read_source(source).with_context(|| format!("reading {}", source.display()))?;
        let result = synthesize::sample(&text, &specs, args.size as usize, args.seed);
        let out_path = if single_file {
            args.output.clone()
        } else {
            out_dir.join(format!("{}.csv", synthesize::output_stem(source)))
        };
        let file =
            File::create(&out_path).with_context(|| format!("creating {}", out_path.display()))?;
        result
            .write_csv(BufWriter::new(file))
            .with_context(|| format!("writing {}", out_path.display()))?;
        println!(
            "Wrote {} row(s) over {} input(s) to {}",
            result.rows.len(),
            result.headers.len(),
            out_path.display()
        );
    }
    Ok(ExitCode::from(exit_code))
}
