mod importer;
mod media;
mod store;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::importer::{ImportOptions, run_import};
use crate::store::{Channel, Store, default_data_dir};

const APP_ID: &str = "dev.koma.App";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("import") {
        return run_import_cli(&args[1..]);
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
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

fn populate_sidebar(list: &gtk4::ListBox, channels: &[Channel], selected_slug: Option<&str>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if channels.is_empty() {
        let empty = gtk4::Label::builder()
            .label("No channels yet")
            .xalign(0.0)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["dim-label"])
            .build();
        list.append(&empty);
        return;
    }

    let mut select_idx = 0i32;
    for (idx, ch) in channels.iter().enumerate() {
        let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        row_box.set_margin_top(6);
        row_box.set_margin_bottom(6);
        row_box.set_margin_start(8);
        row_box.set_margin_end(8);

        let name = gtk4::Label::builder()
            .label(format!("# {}", ch.slug))
            .xalign(0.0)
            .hexpand(true)
            .build();
        let count = gtk4::Label::builder()
            .label(ch.image_count.to_string())
            .css_classes(["dim-label"])
            .build();
        row_box.append(&name);
        row_box.append(&count);

        let row = gtk4::ListBoxRow::builder().child(&row_box).build();
        row.set_widget_name(&ch.slug);
        list.append(&row);

        if selected_slug == Some(ch.slug.as_str()) {
            select_idx = idx as i32;
        }
    }

    if let Some(row) = list.row_at_index(select_idx) {
        list.select_row(Some(&row));
    }
}

fn build_ui(app: &adw::Application) {
    let store = default_data_dir()
        .and_then(Store::open)
        .expect("failed to open koma store");
    let channels = store.list_channels().expect("failed to list channels");
    let active_channel: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(
        channels.first().map(|c| c.slug.clone()),
    ));
    let store = Rc::new(RefCell::new(store));

    let sidebar_list = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    populate_sidebar(
        &sidebar_list,
        &channels,
        active_channel.borrow().as_deref(),
    );

    let sidebar_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&sidebar_list)
        .build();

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&adw::HeaderBar::new());
    sidebar_toolbar.set_content(Some(&sidebar_scroll));

    let sidebar_page = adw::NavigationPage::builder()
        .title("Channels")
        .child(&sidebar_toolbar)
        .build();

    let status = adw::StatusPage::builder()
        .icon_name("image-x-generic-symbolic")
        .title(
            active_channel
                .borrow()
                .as_deref()
                .map(|s| format!("#{s}"))
                .unwrap_or_else(|| "koma".to_string()),
        )
        .description("Press Ctrl+V to paste an image or drop files here.")
        .vexpand(true)
        .build();

    let content_toolbar = adw::ToolbarView::new();
    content_toolbar.add_top_bar(&adw::HeaderBar::new());
    content_toolbar.set_content(Some(&status));

    let content_page = adw::NavigationPage::builder()
        .title("koma")
        .child(&content_toolbar)
        .build();

    sidebar_list.connect_row_selected(glib::clone!(
        #[strong]
        active_channel,
        #[weak]
        status,
        #[weak]
        content_page,
        move |_, row| {
            if let Some(row) = row {
                let slug = row.widget_name().to_string();
                if !slug.is_empty() && slug != "GtkListBoxRow" {
                    *active_channel.borrow_mut() = Some(slug.clone());
                    let title = format!("#{slug}");
                    status.set_title(&title);
                    content_page.set_title(&title);
                }
            }
        }
    ));

    let split_view = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .min_sidebar_width(200.0)
        .max_sidebar_width(280.0)
        .build();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("koma")
        .default_width(1100)
        .default_height(720)
        .content(&split_view)
        .build();

    let key_controller = gtk4::EventControllerKey::new();
    key_controller.connect_key_pressed(glib::clone!(
        #[weak]
        status,
        #[weak]
        sidebar_list,
        #[strong]
        store,
        #[strong]
        active_channel,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, mods| {
            if mods.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(key, gdk::Key::v | gdk::Key::V)
            {
                let Some(channel) = active_channel.borrow().clone() else {
                    status.set_description(Some("Select a channel before pasting."));
                    return glib::Propagation::Stop;
                };
                let clipboard = status.display().clipboard();
                clipboard.read_texture_async(
                    gio::Cancellable::NONE,
                    glib::clone!(
                        #[weak]
                        status,
                        #[weak]
                        sidebar_list,
                        #[strong]
                        store,
                        move |res| match res {
                            Ok(Some(tex)) => {
                                let png = tex.save_to_png_bytes();
                                match store.borrow_mut().ingest_bytes(&channel, png.as_ref(), None) {
                                    Ok(rec) => {
                                        if let Ok(chs) = store.borrow().list_channels() {
                                            populate_sidebar(&sidebar_list, &chs, Some(&channel));
                                        }
                                        status.set_description(Some(&format!(
                                            "Saved {} ({}x{}) to #{channel}",
                                            rec.filename, rec.width, rec.height
                                        )));
                                    }
                                    Err(err) => status.set_description(Some(&format!(
                                        "Failed to save clipboard image: {err}"
                                    ))),
                                }
                            }
                            Ok(None) => {
                                status.set_description(Some("Clipboard has no image texture."))
                            }
                            Err(err) => status.set_description(Some(&format!(
                                "Clipboard read failed: {err}"
                            ))),
                        }
                    ),
                );
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    ));
    window.add_controller(key_controller);

    let drop_target = gtk4::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    drop_target.connect_drop(glib::clone!(
        #[weak]
        status,
        #[weak]
        sidebar_list,
        #[strong]
        store,
        #[strong]
        active_channel,
        #[upgrade_or]
        false,
        move |_, value, _, _| {
            let Some(channel) = active_channel.borrow().clone() else {
                status.set_description(Some("Select a channel before dropping files."));
                return false;
            };
            if let Ok(file_list) = value.get::<gdk::FileList>() {
                let paths: Vec<PathBuf> = file_list
                    .files()
                    .into_iter()
                    .filter_map(|f| f.path())
                    .collect();
                if paths.is_empty() {
                    return false;
                }
                let mut saved = Vec::new();
                let mut errors = Vec::new();
                for path in &paths {
                    match store.borrow_mut().ingest_file(&channel, path) {
                        Ok(rec) => saved.push(rec.filename),
                        Err(err) => errors.push(format!("{}: {err}", path.display())),
                    }
                }
                if !saved.is_empty()
                    && let Ok(chs) = store.borrow().list_channels()
                {
                    populate_sidebar(&sidebar_list, &chs, Some(&channel));
                }
                let msg = if errors.is_empty() {
                    format!("Saved {} file(s) to #{channel}: {}", saved.len(), saved.join(", "))
                } else {
                    format!(
                        "Saved {} file(s), {} failed:\n{}",
                        saved.len(),
                        errors.len(),
                        errors.join("\n")
                    )
                };
                status.set_description(Some(&msg));
                return !saved.is_empty();
            }
            false
        }
    ));
    window.add_controller(drop_target);

    window.present();
}
