use anyhow::{bail, Context, Result};
use liteclip_core::benchmark_harness::{
    parse_scenario_arg, suite_summary_to_json, summarize_benchmark_suite, BenchmarkRunInput,
    BenchmarkSuiteOptions,
};
use liteclip_core::quality_contracts::CANONICAL_PERFORMANCE_SCENARIOS;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    match args[0].as_str() {
        "summarize" => summarize_cmd(&args[1..]),
        "list-scenarios" => {
            list_scenarios_cmd();
            Ok(())
        }
        "--help" | "-h" | "help" => {
            print_usage();
            Ok(())
        }
        other => {
            bail!("Unknown command '{other}'. Use 'benchmark_harness help' for usage details.")
        }
    }
}

fn summarize_cmd(args: &[String]) -> Result<()> {
    let mut run_label: Option<String> = None;
    let mut compare_to_label: Option<String> = None;
    let mut output_path: Option<PathBuf> = None;
    let mut strict_quality_guardrails = false;
    let mut input_specs: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--run-label" => {
                idx += 1;
                run_label = Some(expect_arg_value(args, idx, "--run-label")?.to_string());
            }
            "--compare-to" => {
                idx += 1;
                compare_to_label = Some(expect_arg_value(args, idx, "--compare-to")?.to_string());
            }
            "--out" => {
                idx += 1;
                output_path = Some(PathBuf::from(expect_arg_value(args, idx, "--out")?));
            }
            "--input" => {
                idx += 1;
                input_specs.push(expect_arg_value(args, idx, "--input")?.to_string());
            }
            "--strict-quality" => {
                strict_quality_guardrails = true;
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => bail!("Unknown summarize option '{other}'"),
        }
        idx += 1;
    }

    let run_label =
        run_label.ok_or_else(|| anyhow::anyhow!("missing required --run-label <label>"))?;
    let output_path =
        output_path.ok_or_else(|| anyhow::anyhow!("missing required --out <path>"))?;
    if input_specs.is_empty() {
        bail!(
            "at least one --input is required. Format: --input scenario|stdout_path|stderr_path_or_dash"
        );
    }

    let mut inputs = Vec::with_capacity(input_specs.len());
    for spec in &input_specs {
        inputs.push(parse_input_spec(spec)?);
    }
    let options = BenchmarkSuiteOptions {
        run_label,
        compare_to_label,
        strict_quality_guardrails,
    };
    let summary = summarize_benchmark_suite(&inputs, &options)
        .map_err(|e| anyhow::anyhow!("benchmark summarize failed: {e}"))?;
    let json = suite_summary_to_json(&summary);
    ensure_parent_dir(&output_path)?;
    std::fs::write(&output_path, json)
        .with_context(|| format!("failed writing summary to '{}'", output_path.display()))?;

    println!(
        "Benchmark summary written to {} ({} scenario(s))",
        output_path.display(),
        summary.overall.total_scenarios
    );
    println!(
        "Quality guardrails passed for {} scenario(s); missing telemetry in {} scenario(s).",
        summary.overall.scenarios_with_quality_pass,
        summary.overall.scenarios_with_missing_telemetry
    );
    Ok(())
}

fn parse_input_spec(spec: &str) -> Result<BenchmarkRunInput> {
    let parts: Vec<&str> = spec.split('|').collect();
    if parts.len() < 2 || parts.len() > 3 {
        bail!(
            "invalid --input '{spec}'. Expected format: scenario|stdout_path|stderr_path_or_dash"
        );
    }
    let scenario = parse_scenario_arg(parts[0])
        .map_err(|e| anyhow::anyhow!("invalid scenario in '{spec}': {e}"))?;
    let stdout_path = PathBuf::from(parts[1]);
    let stderr_path = if parts.get(2).is_some_and(|v| *v == "-") || parts.len() < 3 {
        None
    } else {
        Some(PathBuf::from(parts[2]))
    };

    Ok(BenchmarkRunInput {
        label: scenario.as_slug().to_string(),
        scenario,
        stdout_log: stdout_path,
        stderr_log: stderr_path,
    })
}

fn list_scenarios_cmd() {
    println!("Canonical benchmark scenarios:");
    for scenario in &CANONICAL_PERFORMANCE_SCENARIOS {
        let guardrails = if scenario.quality_guardrails.is_empty() {
            "none".to_string()
        } else {
            scenario
                .quality_guardrails
                .iter()
                .map(|g| g.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "- {} | scope={} | warmup={}s | sample={}s | guardrails={}",
            scenario.id.as_slug(),
            scenario.scope.as_str(),
            scenario.warmup_secs,
            scenario.sample_secs,
            guardrails
        );
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create output directory '{}'", parent.display())
            })?;
        }
    }
    Ok(())
}

fn expect_arg_value<'a>(args: &'a [String], idx: usize, flag: &str) -> Result<&'a str> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))
}

fn print_usage() {
    println!(
        "benchmark_harness - scenario-driven benchmark summary over existing telemetry\n\
         \n\
         Commands:\n\
           benchmark_harness list-scenarios\n\
             Prints canonical scenario IDs and contracts.\n\
         \n\
           benchmark_harness summarize --run-label <label> --out <summary.json> --input <spec> [--input <spec> ...] [--compare-to <label>] [--strict-quality]\n\
             Summarizes telemetry logs into stable JSON.\n\
         \n\
         Input spec format:\n\
           scenario|stdout_path|stderr_path_or_dash\n\
         Example:\n\
           benchmark_harness summarize --run-label baseline --out target\\benchmark\\baseline-summary.json \\\n\
             --input active-recording|baseline_active_stdout.log|baseline_active_stderr.log \\\n\
             --input idle-tray|baseline_idle_stdout.log|baseline_idle_stderr.log\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_input_spec_accepts_with_and_without_stderr() {
        let parsed = parse_input_spec(
            "active-recording|baseline_active_stdout.log|baseline_active_stderr.log",
        )
        .expect("input with stderr");
        assert_eq!(parsed.scenario.as_slug(), "active-recording");
        assert_eq!(
            parsed.stderr_log.as_deref(),
            Some(Path::new("baseline_active_stderr.log"))
        );

        let parsed_no_stderr =
            parse_input_spec("idle-tray|baseline_idle_stdout.log|-").expect("input without stderr");
        assert_eq!(parsed_no_stderr.scenario.as_slug(), "idle-tray");
        assert!(parsed_no_stderr.stderr_log.is_none());
    }

    #[test]
    fn parse_input_spec_rejects_invalid_formats() {
        assert!(parse_input_spec("active-recording").is_err());
        assert!(parse_input_spec("unknown|stdout.log|-").is_err());
    }
}
