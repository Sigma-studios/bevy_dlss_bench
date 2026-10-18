//! bevy_dlss_bench: screenshots of the Bevy rendering features that break when DLSS
//! renders the main pass at a lower resolution (`MainPassResolutionOverride`) while other
//! passes still use the full-size `view.viewport`.
//!
//! The orchestrator re-runs this binary once per (scene, variant) with `--capture`, because a
//! Bevy app (and its render device / DLSS context) cannot be rebuilt inside one process.
//! It then composes the comparison sheets from the PNGs.
//!
//! `--view` opens the interactive viewer instead (see `viewer.rs`).

mod capture;
mod meta;
mod scenes;
mod sheet;
mod variant;
mod viewer;
mod world;

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use bevy::{app::AppExit, math::UVec2};
use clap::Parser;

use crate::{
    capture::{EXIT_DLSS_UNSUPPORTED, Job},
    meta::{Meta, parse_res},
    scenes::SceneKind,
    sheet::{Entry, SceneResult},
    variant::{DlssMode, Variant},
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Scene(s) to capture: `all` or a comma-separated list (see --list).
    #[arg(long, default_value = "all")]
    scene: String,

    /// Variants to capture: `all`, or a comma-separated list of variant names
    /// (`native`, `dlss-quality`, `dlss-quality-forward`, ...) or kinds
    /// (`native`, `taa`, `dlss`, `forward`, `dlss-taa`).
    #[arg(long, default_value = "all")]
    variants: String,

    /// DLSS quality modes for the `dlss-*` variants, comma-separated
    /// (dlaa, quality, balanced, performance, ultra-performance).
    /// The first one is also used for the `-forward` and `-taa` combinations.
    #[arg(long, default_value = "quality,performance")]
    mode: String,

    /// Output resolution of every capture.
    #[arg(long, default_value = "1920x1080", value_parser = parse_res_arg)]
    res: UVec2,

    /// Frames to render after every pipeline is ready, before capturing
    /// (lets DLSS / TAA history converge).
    #[arg(long, default_value_t = 90)]
    settle: u32,

    /// Output directory.
    #[arg(long, default_value = "out")]
    out: PathBuf,

    /// Maximum width of one tile in the per-scene sheets.
    #[arg(long, default_value_t = 960)]
    tile_width: u32,

    /// Only capture, don't compose sheets.
    #[arg(long)]
    no_sheets: bool,

    /// Only compose sheets from the existing captures in --out.
    #[arg(long)]
    sheets_only: bool,

    /// Open the interactive viewer on the first scene of --scene, starting with the first
    /// variant of --variants if it is an exact variant name (else native).
    #[arg(long)]
    view: bool,

    /// List scenes and their variants, then exit.
    #[arg(long)]
    list: bool,

    /// Internal: capture a single `<scene>:<variant>` in this process.
    #[arg(long, hide = true)]
    capture: Option<String>,
}

fn parse_res_arg(s: &str) -> Result<UVec2, String> {
    parse_res(s)
        .filter(|r| r.x >= 64 && r.y >= 64)
        .ok_or_else(|| format!("expected WxH (e.g. 1920x1080), got `{s}`"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let modes = cli
        .mode
        .split(',')
        .map(|s| DlssMode::parse(s.trim()).ok_or_else(|| format!("unknown DLSS mode `{s}`")))
        .collect::<Result<Vec<_>, _>>()?;

    if let Some(spec) = &cli.capture {
        let (scene, variant) = spec.split_once(':').ok_or("--capture expects <scene>:<variant>")?;
        let scene = SceneKind::parse(scene).ok_or_else(|| format!("unknown scene `{scene}`"))?;
        let variant = Variant::parse(variant).ok_or_else(|| format!("unknown variant `{variant}`"))?;
        let job = Job {
            scene,
            variant,
            res: cli.res,
            settle: cli.settle,
            scene_dir: cli.out.join(scene.info().name),
        };
        return Ok(match capture::run(job) {
            AppExit::Success => ExitCode::SUCCESS,
            AppExit::Error(code) => ExitCode::from(code.get()),
        });
    }

    let scenes = select_scenes(&cli.scene)?;
    if cli.view {
        let variant = cli
            .variants
            .split(',')
            .next()
            .and_then(|v| Variant::parse(v.trim()))
            .unwrap_or(Variant::NATIVE);
        return Ok(match viewer::run(scenes[0], variant, cli.res, cli.out.clone()) {
            AppExit::Success => ExitCode::SUCCESS,
            AppExit::Error(code) => ExitCode::from(code.get()),
        });
    }
    let plan: Vec<(SceneKind, Vec<Variant>)> = scenes
        .iter()
        .map(|&s| (s, filter_variants(s.variants(&modes), &cli.variants)))
        .collect();

    if cli.list {
        for (scene, variants) in &plan {
            let info = scene.info();
            println!("{:<18} {}", info.name, info.title);
            println!("{:<18} feature : {}", "", info.feature);
            println!("{:<18} expected: {}", "", info.expected);
            let names: Vec<String> = variants.iter().map(Variant::name).collect();
            println!("{:<18} variants: {}\n", "", names.join(", "));
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut failures = 0;
    if !cli.sheets_only {
        failures = capture_all(&cli, &plan)?;
    }

    let results: Vec<SceneResult> = plan
        .iter()
        .map(|(scene, variants)| collect(&cli.out, *scene, variants))
        .collect();
    write_summary(&cli.out, &results).map_err(|e| e.to_string())?;

    if !cli.no_sheets {
        let output = (cli.res.x, cli.res.y);
        for result in &results {
            let path = cli.out.join(result.kind.info().name).join("sheet.png");
            sheet::scene_sheet(result, output, cli.tile_width, &path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            println!("sheet: {}", path.display());
        }
        let index = cli.out.join("index.png");
        sheet::index_sheet(&results, output, &index).map_err(|e| e.to_string())?;
        println!("index: {}", index.display());
    }
    println!("summary: {}", cli.out.join("SUMMARY.md").display());

    Ok(if failures > 0 {
        eprintln!("{failures} capture(s) failed");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn select_scenes(spec: &str) -> Result<Vec<SceneKind>, String> {
    if spec == "all" {
        return Ok(SceneKind::ALL.to_vec());
    }
    spec.split(',')
        .map(|s| {
            SceneKind::parse(s.trim()).ok_or_else(|| {
                let names: Vec<_> = SceneKind::ALL.iter().map(|k| k.info().name).collect();
                format!("unknown scene `{s}` (expected all or one of {})", names.join(", "))
            })
        })
        .collect()
}

fn filter_variants(all: Vec<Variant>, spec: &str) -> Vec<Variant> {
    if spec == "all" {
        return all;
    }
    let tokens: Vec<&str> = spec.split(',').map(str::trim).collect();
    all.into_iter()
        .filter(|v| {
            tokens.iter().any(|&t| {
                t == v.name()
                    || match t {
                        "native" => v.dlss.is_none() && !v.taa,
                        "taa" => v.dlss.is_none() && v.taa,
                        "dlss" => v.dlss.is_some() && !v.forward && !v.taa,
                        "forward" => v.dlss.is_some() && v.forward,
                        "dlss-taa" => v.dlss.is_some() && v.taa,
                        _ => false,
                    }
            })
        })
        .collect()
}

/// Runs one child process per capture. Returns the number of failed captures.
fn capture_all(cli: &Cli, plan: &[(SceneKind, Vec<Variant>)]) -> Result<usize, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut dlss_unsupported: Option<String> = None;
    let mut failures = 0;
    let total: usize = plan.iter().map(|(_, v)| v.len()).sum();
    let mut n = 0;

    for (scene, variants) in plan {
        for variant in variants {
            n += 1;
            let job = Job {
                scene: *scene,
                variant: *variant,
                res: cli.res,
                settle: cli.settle,
                scene_dir: cli.out.join(scene.info().name),
            };
            for stale in [job.labelled_path(), job.raw_path(), job.meta_path()] {
                let _ = fs::remove_file(stale);
            }

            // DLSS support doesn't change between captures: don't relaunch to find out again.
            if variant.dlss.is_some()
                && let Some(status) = &dlss_unsupported
            {
                println!("[{n}/{total}] {}/{variant}: skipped ({status})", scene.info().name);
                let mut meta = placeholder_meta(&job);
                meta.status = status.clone();
                meta.write(&job.meta_path());
                continue;
            }

            println!("[{n}/{total}] {}/{variant}", scene.info().name);
            let status = Command::new(&exe)
                .arg("--capture")
                .arg(format!("{}:{}", scene.info().name, variant))
                .arg("--res")
                .arg(format!("{}x{}", cli.res.x, cli.res.y))
                .arg("--settle")
                .arg(cli.settle.to_string())
                .arg("--out")
                .arg(&cli.out)
                .status()
                .map_err(|e| format!("cannot launch capture process: {e}"))?;

            match status.code() {
                Some(0) => {}
                Some(c) if c == EXIT_DLSS_UNSUPPORTED as i32 => {
                    let status = Meta::read(&job.meta_path())
                        .map(|m| m.status)
                        .unwrap_or_else(|| "dlss-unsupported".into());
                    eprintln!("DLSS unsupported here: {status}. Remaining DLSS variants are skipped.");
                    dlss_unsupported = Some(status);
                }
                code => {
                    failures += 1;
                    let mut meta = placeholder_meta(&job);
                    meta.status = format!("failed: capture process exited with {code:?}");
                    meta.write(&job.meta_path());
                    eprintln!("{}/{variant}: {}", scene.info().name, meta.status);
                }
            }
        }
    }
    Ok(failures)
}

fn placeholder_meta(job: &Job) -> Meta {
    let info = job.scene.info();
    Meta {
        scene: info.name.into(),
        title: info.title.into(),
        feature: info.feature.into(),
        variant: job.variant.name(),
        variant_desc: job.variant.describe(),
        expected: job.scene.expected_for(&job.variant).into(),
        pipeline: if job.scene.uses_deferred(&job.variant) { "deferred" } else { "forward" }.into(),
        output: job.res,
        ..Default::default()
    }
}

fn collect(out: &Path, scene: SceneKind, variants: &[Variant]) -> SceneResult {
    let dir = out.join(scene.info().name);
    let existing = |p: PathBuf| p.exists().then_some(p);
    SceneResult {
        kind: scene,
        entries: variants
            .iter()
            .map(|v| Entry {
                variant: *v,
                meta: Meta::read(&dir.join(format!("{v}.meta"))),
                labelled: existing(dir.join(format!("{v}.png"))),
                raw: existing(dir.join("raw").join(format!("{v}.png"))),
            })
            .collect(),
    }
}

fn write_summary(out: &Path, results: &[SceneResult]) -> std::io::Result<()> {
    let mut md = String::from(
        "# bevy_dlss_bench results\n\n| scene | variant | status | render -> output | pipeline | expected |\n\
         |---|---|---|---|---|---|\n",
    );
    let mut adapter = None;
    for r in results {
        for e in &r.entries {
            let (status, res, pipeline, expected) = match &e.meta {
                Some(m) => {
                    adapter = adapter.or(Some(m.adapter.clone()).filter(|a| !a.is_empty()));
                    let res = match m.render {
                        Some(rr) => format!("{}x{} -> {}x{}", rr.x, rr.y, m.output.x, m.output.y),
                        None => String::from("-"),
                    };
                    (m.status.clone(), res, m.pipeline.clone(), m.expected.clone())
                }
                None => ("not captured".into(), "-".into(), "-".into(), "-".into()),
            };
            let _ = writeln!(
                md,
                "| {} | {} | {status} | {res} | {pipeline} | {expected} |",
                r.kind.info().name,
                e.variant
            );
        }
    }
    if let Some(a) = adapter {
        let _ = write!(md, "\nAdapter: {a}\n");
    }
    fs::create_dir_all(out)?;
    fs::write(out.join("SUMMARY.md"), md)
}
