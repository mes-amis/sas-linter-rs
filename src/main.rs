use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sas_linter::encoding::read_source;
use sas_linter::value_specs::{SpecSource, ValueSpecs};
use sas_linter::{catalog_check, rules, synthesize, Config, Formatter, Linter};

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
    /// is drawn from one of five strategies, mixed by --weights: baseline
    /// (everything at default), single (satisfy one predicate), combo
    /// (satisfy --combo-size predicates at once), perturbation (baseline
    /// with each cell flipped to random at --perturbation-prob), and chaos
    /// (uniform sampling).
    ///
    /// --minimal replaces the weighted mix with the smallest row set it
    /// can find that drives every `if` both ways (100% branch coverage),
    /// sorted by the first branch each row reaches in source order.
    ///
    /// Deterministic for a given --seed.
    Synthesize(SynthesizeArgs),

    /// Check that each acceptable-values catalog row agrees with itself.
    ///
    /// Compares every row's values column (`1-3,8`) with its vector column
    /// (`1,2,3,8`) and reports rows that name different values, or where
    /// only one of the two is a value list (often a shifted column). Reads
    /// the catalog files and column options from the config's
    /// `variable_value_out_of_known_range` entry unless CSVs are given.
    CheckCatalog(CheckCatalogArgs),
}

#[derive(clap::Args, Debug)]
struct CheckCatalogArgs {
    /// YAML config whose `variable_value_out_of_known_range` options
    /// locate the catalog and name its columns. Missing file → defaults.
    #[arg(long, default_value = "config/lint.yaml")]
    config: PathBuf,

    /// Vector column name. Overrides the config's `vector_column`.
    #[arg(long)]
    vector_column: Option<String>,

    /// Catalog CSVs to check instead of the config's `csv_paths`.
    csv_paths: Vec<PathBuf>,
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

    /// Strategy mix as `name=weight,…` over baseline, single, combo,
    /// perturbation, and chaos. Weights are relative (they need not sum
    /// to 1); strategies left out get 0. E.g. `--weights single=3,combo=1`.
    #[arg(long, value_name = "MIX", default_value_t = synthesize::Weights::default())]
    weights: synthesize::Weights,

    /// Chance (0–1) that each cell of a perturbation row is redrawn at
    /// random.
    #[arg(long, value_name = "P", default_value_t = synthesize::DEFAULT_PERTURBATION_PROB,
          value_parser = parse_probability)]
    perturbation_prob: f64,

    /// Predicates satisfied per combo row: `MIN-MAX`, or `N` for exactly N.
    #[arg(long, value_name = "RANGE", default_value_t = synthesize::ComboSizes::default())]
    combo_size: synthesize::ComboSizes,

    /// Emit the fewest rows covering every `if` branch outcome, in
    /// execution order, instead of --size weighted samples. Outcomes no
    /// row can reach are reported on stderr.
    #[arg(long, conflicts_with_all = ["size", "weights", "perturbation_prob", "combo_size"])]
    minimal: bool,

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

fn parse_probability(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(p) if (0.0..=1.0).contains(&p) => Ok(p),
        _ => Err(format!("`{s}` is not a probability between 0 and 1")),
    }
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
    if let Some(Command::CheckCatalog(args)) = &cli.command {
        return run_check_catalog(args);
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
        let out_path = if single_file {
            args.output.clone()
        } else {
            out_dir.join(format!("{}.csv", synthesize::output_stem(source)))
        };
        let (result, coverage) = if args.minimal {
            let (result, coverage) = synthesize::minimal(&text, &specs, args.seed);
            (result, Some(coverage))
        } else {
            let mix = synthesize::Mix {
                weights: args.weights.clone(),
                perturbation_prob: args.perturbation_prob,
                combo_sizes: args.combo_size.clone(),
            };
            let result =
                synthesize::sample_with(&text, &specs, args.size as usize, args.seed, &mix);
            (result, None)
        };
        let file =
            File::create(&out_path).with_context(|| format!("creating {}", out_path.display()))?;
        result
            .write_csv(BufWriter::new(file))
            .with_context(|| format!("writing {}", out_path.display()))?;
        let covers = match &coverage {
            Some(c) => {
                let mut parts = vec![format!(
                    "covers {}/{} branch outcomes",
                    c.covered(),
                    c.total
                )];
                if c.values > 0 {
                    parts.push(format!(
                        "{}/{} assigned values",
                        c.values - c.unproduced.len(),
                        c.values
                    ));
                }
                if c.boundaries > 0 {
                    parts.push(format!("{} boundary values", c.boundaries));
                }
                format!(" ({})", parts.join(", "))
            }
            None => String::new(),
        };
        println!(
            "Wrote {} row(s) over {} input(s) to {}{covers}",
            result.rows.len(),
            result.headers.len(),
            out_path.display()
        );
        for (var, value) in coverage.iter().flat_map(|c| &c.unproduced) {
            eprintln!(
                "sas-lint synthesize: {}: no row gives {var} = {value}",
                source.display()
            );
        }
        for (line, outcome) in coverage.iter().flat_map(|c| &c.uncovered) {
            eprintln!(
                "sas-lint synthesize: {}:{line}: no row reaches the {} branch",
                source.display(),
                if *outcome { "true" } else { "false" }
            );
        }
    }
    Ok(ExitCode::from(exit_code))
}

fn run_check_catalog(args: &CheckCatalogArgs) -> Result<ExitCode> {
    let cfg = Config::load(&args.config)?;
    let mut source = match cfg.rules.get("variable_value_out_of_known_range") {
        Some(opts) => SpecSource::from_opts(opts)?,
        None => SpecSource::default(),
    };
    if !args.csv_paths.is_empty() {
        source.csv_paths = args.csv_paths.clone();
    }
    if let Some(column) = &args.vector_column {
        source.vector_column = column.clone();
    }
    if source.csv_paths.is_empty() {
        eprintln!(
            "sas-lint check-catalog: no catalog CSVs given and none configured in {}",
            args.config.display()
        );
        return Ok(ExitCode::from(2));
    }

    let mut exit_code: u8 = 0;
    for path in &source.csv_paths {
        if !path.is_file() {
            eprintln!("sas-lint check-catalog: {}: no such file", path.display());
            exit_code = 2;
            continue;
        }
        let issues = match catalog_check::check_file(path, &source) {
            Ok(issues) => issues,
            Err(err) => {
                eprintln!("sas-lint check-catalog: {err:#}");
                exit_code = 2;
                continue;
            }
        };
        if !issues.is_empty() {
            exit_code = exit_code.max(1);
        }
        for issue in issues {
            println!("{}:{issue}", path.display());
        }
    }
    Ok(ExitCode::from(exit_code))
}
