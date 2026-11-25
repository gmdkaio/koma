mod importer;
mod media;
mod store;
mod ui;
mod zip;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::importer::{ImportOptions, run_import};
use crate::store::{Store, default_data_dir};

const APP_ID: &str = "dev.koma.App";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("import") {
        return run_import_cli(&args[1..]);
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::build_ui);
    if app.run() == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_import_cli(args: &[String]) -> ExitCode {
    let mut source_dir: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut merges = HashMap::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry_run = true,
            "--merge" => {
                let Some(pair) = args.get(i + 1) else {
                    eprintln!("koma import: --merge requires <from>=<to>");
                    return ExitCode::FAILURE;
                };
                let Some((from, to)) = pair.split_once('=') else {
                    eprintln!("koma import: invalid --merge value {pair:?}, expected <from>=<to>");
                    return ExitCode::FAILURE;
                };
                merges.insert(from.to_string(), to.to_string());
                i += 1;
            }
            "--data-dir" => {
                let Some(dir) = args.get(i + 1) else {
                    eprintln!("koma import: --data-dir requires a path");
                    return ExitCode::FAILURE;
                };
                data_dir = Some(PathBuf::from(dir));
                i += 1;
            }
            flag if flag.starts_with('-') => {
                eprintln!("koma import: unknown flag {flag}");
                return ExitCode::FAILURE;
            }
            path if source_dir.is_none() => source_dir = Some(PathBuf::from(path)),
            extra => {
                eprintln!("koma import: unexpected argument {extra}");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let Some(source_dir) = source_dir else {
        eprintln!(
            "usage: koma import <source_dir> [--dry-run] [--merge <from>=<to>]... [--data-dir <dir>]"
        );
        return ExitCode::FAILURE;
    };

    let opts = ImportOptions {
        source_dir,
        dry_run,
        merges,
    };

    let result = if dry_run {
        run_import(None, &opts)
    } else {
        let root = match data_dir.map(Ok).unwrap_or_else(default_data_dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("koma import: {e}");
                return ExitCode::FAILURE;
            }
        };
        match Store::open(root) {
            Ok(mut store) => {
                let root_display = store.root().display().to_string();
                let res = run_import(Some(&mut store), &opts);
                if res.is_ok() {
                    println!("Data dir: {root_display}");
                }
                res
            }
            Err(e) => Err(e),
        }
    };

    match result {
        Ok(report) => {
            let mode = if dry_run { "Dry run" } else { "Imported" };
            for (channel, count) in &report.channels {
                println!("  #{channel:<16} {count}");
            }
            if !report.skipped_empty_dirs.is_empty() {
                println!(
                    "Skipped empty folders: {}",
                    report.skipped_empty_dirs.join(", ")
                );
            }
            let media_mb = report.total_media_bytes as f64 / (1024.0 * 1024.0);
            if dry_run {
                println!(
                    "{mode}: {} images across {} channels ({media_mb:.1} MB)",
                    report.imported,
                    report.channels.len()
                );
            } else {
                let thumb_mb = report.total_thumb_bytes as f64 / (1024.0 * 1024.0);
                println!(
                    "{mode}: {} images across {} channels ({media_mb:.1} MB media, {thumb_mb:.1} MB WebP thumbs)",
                    report.imported,
                    report.channels.len()
                );
            }
            if !report.failures.is_empty() {
                eprintln!("Failures ({}):", report.failures.len());
                for f in &report.failures {
                    eprintln!("  {}: {}", f.path.display(), f.reason);
                }
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("koma import: {e}");
            ExitCode::FAILURE
        }
    }
}
