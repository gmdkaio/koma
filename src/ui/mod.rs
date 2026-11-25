mod grid;
mod inspector;
mod lightbox;
mod media_util;
mod switcher;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::store::{Channel, ImageRecord, Store, default_data_dir};
use crate::ui::grid::GridCanvas;
use crate::ui::inspector::InspectorPane;
use crate::ui::lightbox::LightboxView;
use crate::ui::media_util::{AsyncThumbLoader, ThumbCache, load_texture};
use crate::ui::switcher::{SwitcherAction, SwitcherMode, present_switcher};
use crate::zip::write_stored_zip;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveView {
    Home,
    Channel(i64),
}

struct AppCtx {
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    /// Switches right content area between main gallery and Lightbox while keeping sidebar open.
    content_stack: gtk4::Stack,
    store: Rc<RefCell<Store>>,
    thumb_cache: Rc<RefCell<ThumbCache>>,
    channels: RefCell<Vec<Channel>>,
    active_view: Cell<ActiveView>,
    view_mode: RefCell<String>,
    selected_images: RefCell<Vec<ImageRecord>>,
    focused_image: RefCell<Option<ImageRecord>>,
    select_mode: Cell<bool>,
    suppress_sidebar_select: Cell<bool>,

    // Sidebar widgets
    left_split: adw::OverlaySplitView,
    sidebar_toggle_btn: gtk4::ToggleButton,
    home_row_btn: gtk4::Button,
    channel_list: gtk4::ListBox,
    footer_count: gtk4::Label,

    // Topbar widgets
    title_hash: gtk4::Label,
    title_name: gtk4::Label,
    title_sub: gtk4::Label,
    upload_btn: gtk4::Button,
    grid_toggle: gtk4::ToggleButton,
    timeline_toggle: gtk4::ToggleButton,
    select_toggle: gtk4::ToggleButton,
    inspector_split: adw::OverlaySplitView,
    inspector_toggle: gtk4::ToggleButton,

    // Multi-select floating bar (hidden unless multi-selecting)
    batch_bar: gtk4::Box,
    batch_count_label: gtk4::Label,

    // Keep FileMonitor alive for the lifetime of the window so live gtk.css reloads work.
    _theme_monitor: Option<gio::FileMonitor>,

    // Child views
    grid_canvas: RefCell<Option<Rc<GridCanvas>>>,
    inspector: RefCell<Option<Rc<InspectorPane>>>,
    lightbox: RefCell<Option<Rc<LightboxView>>>,
}

impl AppCtx {
    fn toast(&self, msg: &str) {
        let toast = adw::Toast::builder().title(msg).timeout(3).build();
        self.toast_overlay.add_toast(toast);
    }

    fn active_channel(&self) -> Option<Channel> {
        match self.active_view.get() {
            ActiveView::Home => None,
            ActiveView::Channel(id) => self
                .channels
                .borrow()
                .iter()
                .find(|c| c.id == id)
                .cloned(),
        }
    }

    fn reload_all(self: &Rc<Self>) {
        let channels = match self.store.borrow().list_channels() {
            Ok(c) => c,
            Err(e) => {
                self.toast(&format!("Failed to load channels: {e}"));
                return;
            }
        };

        let total_files: i64 = channels.iter().map(|c| c.image_count).sum();
        self.footer_count
            .set_label(&format!("koma/ · {} files", format_int(total_files)));

        if let ActiveView::Channel(id) = self.active_view.get()
            && !channels.iter().any(|c| c.id == id)
        {
            self.active_view.set(
                channels
                    .first()
                    .map(|c| ActiveView::Channel(c.id))
                    .unwrap_or(ActiveView::Home),
            );
        }

        *self.channels.borrow_mut() = channels;
        self.rebuild_sidebar();
        self.reload_images();
    }

    fn rebuild_sidebar(self: &Rc<Self>) {
        self.suppress_sidebar_select.set(true);
        while let Some(child) = self.channel_list.first_child() {
            self.channel_list.remove(&child);
        }

        let channels = self.channels.borrow().clone();
        let active = self.active_view.get();

        if active == ActiveView::Home {
            self.home_row_btn.add_css_class("koma-home-active");
        } else {
            self.home_row_btn.remove_css_class("koma-home-active");
        }

        let mut selected_idx: Option<i32> = None;
        for (idx, ch) in channels.iter().enumerate() {
            let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
            row_box.set_margin_top(5);
            row_box.set_margin_bottom(5);
            row_box.set_margin_start(10);
            row_box.set_margin_end(9);

            let hash = gtk4::Label::builder()
                .label("#")
                .css_classes(["koma-channel-hash"])
                .build();
            let name = gtk4::Label::builder()
                .label(&ch.slug)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();
            let count = gtk4::Label::builder()
                .label(ch.image_count.to_string())
                .css_classes(["koma-tabular"])
                .build();
            row_box.append(&hash);
            row_box.append(&name);
            row_box.append(&count);

            let row = gtk4::ListBoxRow::builder().child(&row_box).build();
            row.set_widget_name(&ch.id.to_string());

            let right_click = gtk4::GestureClick::builder().button(3).build();
            let ctx_rc = self.clone();
            let ch_rc = ch.clone();
            right_click.connect_released(move |gesture, _, x, y| {
                if let Some(widget) = gesture.widget() {
                    ctx_rc.show_channel_menu(&widget, x as i32, y as i32, ch_rc.clone());
                }
            });
            row.add_controller(right_click);

            self.channel_list.append(&row);
            if active == ActiveView::Channel(ch.id) {
                selected_idx = Some(idx as i32);
            }
        }

        if let Some(idx) = selected_idx
            && let Some(row) = self.channel_list.row_at_index(idx)
        {
            self.channel_list.select_row(Some(&row));
        } else {
            self.channel_list.unselect_all();
        }
        self.suppress_sidebar_select.set(false);
    }

    fn reload_images(self: &Rc<Self>) {
        let active = self.active_view.get();
        let (channel_id, title, sub, is_home) = match active {
            ActiveView::Home => (None, "Home".to_string(), "Recent".to_string(), true),
            ActiveView::Channel(id) => {
                let ch = self.channels.borrow().iter().find(|c| c.id == id).cloned();
                match ch {
                    Some(c) => (Some(c.id), c.slug.clone(), c.image_count.to_string(), false),
                    None => (None, "Home".to_string(), "Recent".to_string(), true),
                }
            }
        };

        self.title_hash.set_visible(!is_home);
        self.title_name.set_label(&title);
        self.title_sub.set_label(&sub);
        self.upload_btn.set_sensitive(!is_home);

        let records = match self.store.borrow().list_images(channel_id) {
            Ok(r) => r,
            Err(e) => {
                self.toast(&format!("Failed to load images: {e}"));
                return;
            }
        };

        let mode = self.view_mode.borrow().clone();
        if let Some(canvas) = self.grid_canvas.borrow().as_ref() {
            canvas.set_records(records, &mode, is_home);
            gtk4::prelude::GtkWindowExt::set_focus(&self.window, Some(&canvas.focus_widget()));
        }
        self.update_batch_bar();
    }

    fn select_view(self: &Rc<Self>, view: ActiveView) {
        self.content_stack.set_visible_child_name("gallery");
        self.active_view.set(view);
        self.select_mode.set(false);
        self.select_toggle.set_active(false);

        let default_mode = match view {
            ActiveView::Home => "timeline",
            ActiveView::Channel(_) => "grid",
        };
        *self.view_mode.borrow_mut() = default_mode.to_string();
        self.grid_toggle.set_active(default_mode == "grid");
        self.timeline_toggle.set_active(default_mode == "timeline");

        if self.left_split.is_collapsed() {
            self.left_split.set_show_sidebar(false);
            self.sidebar_toggle_btn.set_active(false);
        }

        self.rebuild_sidebar();
        self.reload_images();
    }

    fn toggle_inspector_for(&self, rec: ImageRecord) {
        let already_open_same = self.inspector_split.shows_sidebar()
            && self
                .inspector
                .borrow()
                .as_ref()
                .and_then(|ins| ins.current_id())
                == Some(rec.id);

        if already_open_same {
            self.inspector_split.set_show_sidebar(false);
            self.inspector_toggle.set_active(false);
            return;
        }

        *self.focused_image.borrow_mut() = Some(rec.clone());
        if let Some(ins) = self.inspector.borrow().as_ref() {
            ins.set_image(Some(rec), &self.channels.borrow());
        }
        self.inspector_split.set_show_sidebar(true);
        self.inspector_toggle.set_active(true);
    }

    fn update_batch_bar(&self) {
        let count = self.selected_images.borrow().len();
        let show_batch = count > 1 || (self.select_mode.get() && count >= 1);
        if show_batch {
            self.batch_count_label
                .set_label(&format!("{count} selected"));
        }
        self.batch_bar.set_visible(show_batch);
    }

    fn target_ids_for_action(&self) -> Vec<i64> {
        let sel = self.selected_images.borrow();
        if !sel.is_empty() {
            return sel.iter().map(|r| r.id).collect();
        }
        self.focused_image
            .borrow()
            .as_ref()
            .map(|r| vec![r.id])
            .unwrap_or_default()
    }

    fn copy_image_to_clipboard(&self, rec: &ImageRecord) {
        let abs = self.store.borrow().root().join(&rec.rel_path);
        match load_texture(&abs) {
            Some(tex) => {
                gtk4::prelude::WidgetExt::display(&self.window)
                    .clipboard()
                    .set_texture(&tex);
                self.toast(&format!("Copied {}", rec.filename));
            }
            None => self.toast(&format!("Could not decode {} for clipboard", rec.filename)),
        }
    }

    fn paste_from_clipboard(self: &Rc<Self>) {
        let Some(ch) = self.active_channel() else {
            self.toast("Select a channel first before pasting");
            return;
        };
        let clipboard = gtk4::prelude::WidgetExt::display(&self.window).clipboard();
        let ctx = self.clone();
        clipboard.read_texture_async(gio::Cancellable::NONE, move |res| match res {
            Ok(Some(tex)) => {
                let png = tex.save_to_png_bytes();
                let res = ctx
                    .store
                    .borrow_mut()
                    .ingest_bytes(&ch.slug, png.as_ref(), None);
                match res {
                    Ok(rec) => {
                        ctx.toast(&format!("Saved {} to #{}", rec.filename, ch.slug));
                        ctx.reload_all();
                    }
                    Err(e) => ctx.toast(&format!("Paste failed: {e}")),
                }
            }
            Ok(None) => ctx.toast("Clipboard does not contain an image"),
            Err(e) => ctx.toast(&format!("Clipboard read failed: {e}")),
        });
    }

    fn ingest_paths(self: &Rc<Self>, paths: &[PathBuf]) {
        let Some(ch) = self.active_channel() else {
            self.toast("Select a channel first before uploading files");
            return;
        };
        let mut saved = 0usize;
        let mut failed = 0usize;
        for path in paths {
            match self.store.borrow_mut().ingest_file(&ch.slug, path) {
                Ok(_) => saved += 1,
                Err(_) => failed += 1,
            }
        }
        if saved > 0 {
            self.reload_all();
        }
        if failed == 0 && saved > 0 {
            self.toast(&format!("Added {saved} image(s) to #{}", ch.slug));
        } else if failed > 0 {
            self.toast(&format!(
                "Added {saved}, {failed} skipped (not supported images)"
            ));
        }
    }

    fn open_upload_dialog(self: &Rc<Self>) {
        if self.active_channel().is_none() {
            self.toast("Select a channel first");
            return;
        }
        let dialog = gtk4::FileDialog::builder().title("Upload Images").build();
        let ctx = self.clone();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(list) = res {
                let mut paths = Vec::new();
                for i in 0..list.n_items() {
                    if let Some(file) = list.item(i).and_downcast::<gio::File>()
                        && let Some(path) = file.path()
                    {
                        paths.push(path);
                    }
                }
                if !paths.is_empty() {
                    ctx.ingest_paths(&paths);
                }
            }
        });
    }

    fn export_selected_zip(self: &Rc<Self>) {
        let items: Vec<ImageRecord> = {
            let sel = self.selected_images.borrow();
            if !sel.is_empty() {
                sel.clone()
            } else if let Some(f) = self.focused_image.borrow().clone() {
                vec![f]
            } else {
                Vec::new()
            }
        };
        if items.is_empty() {
            self.toast("No images selected to export");
            return;
        }

        let default_name = match self.active_channel() {
            Some(c) => format!("koma-{}.zip", c.slug),
            None => "koma-export.zip".to_string(),
        };
        let dialog = gtk4::FileDialog::builder()
            .title("Export Selected as Zip")
            .initial_name(&default_name)
            .build();
        let ctx = self.clone();
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(dest) = file.path() else { return };
            let root = ctx.store.borrow().root().to_path_buf();
            let owned: Vec<(String, PathBuf)> = items
                .iter()
                .map(|r| (r.filename.clone(), root.join(&r.rel_path)))
                .collect();
            let refs: Vec<(String, &std::path::Path)> = owned
                .iter()
                .map(|(n, p)| (n.clone(), p.as_path()))
                .collect();
            match write_stored_zip(&dest, &refs) {
                Ok(()) => ctx.toast(&format!(
                    "Exported {} image(s) to {}",
                    refs.len(),
                    dest.display()
                )),
                Err(e) => ctx.toast(&format!("Export failed: {e}")),
            }
        });
    }

    fn confirm_delete_selected(self: &Rc<Self>) {
        let ids = self.target_ids_for_action();
        if ids.is_empty() {
            return;
        }
        let alert = adw::AlertDialog::builder()
            .heading(format!("Delete {} image(s)?", ids.len()))
            .body("This removes the selected images from koma's managed folder and database.")
            .build();
        alert.add_response("cancel", "Cancel");
        alert.add_response("delete", "Delete");
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");

        let ctx = self.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |resp| {
            if resp == "delete" {
                match ctx.store.borrow_mut().delete_images(&ids) {
                    Ok(n) => {
                        ctx.select_mode.set(false);
                        ctx.select_toggle.set_active(false);
                        ctx.toast(&format!("Deleted {n} image(s)"));
                        ctx.reload_all();
                    }
                    Err(e) => ctx.toast(&format!("Delete failed: {e}")),
                }
            }
        });
    }

    fn open_switcher(self: &Rc<Self>, mode: SwitcherMode, explicit_ids: Option<Vec<i64>>) {
        let channels = self.channels.borrow().clone();
        let root_dir = self.store.borrow().root().to_path_buf();
        let ctx = self.clone();
        present_switcher(
            &self.window,
            root_dir,
            self.thumb_cache.clone(),
            channels,
            mode,
            move |action| match action {
                SwitcherAction::OpenChannel(id) => {
                    ctx.select_view(ActiveView::Channel(id));
                }
                SwitcherAction::MoveToChannel(target_id) => {
                    let ids = explicit_ids
                        .clone()
                        .unwrap_or_else(|| ctx.target_ids_for_action());
                    ctx.move_images_to(ids, target_id);
                }
                SwitcherAction::CreateChannel(slug, also_move) => {
                    match ctx.store.borrow().create_channel(&slug) {
                        Ok(ch) => {
                            if also_move {
                                let ids = explicit_ids
                                    .clone()
                                    .unwrap_or_else(|| ctx.target_ids_for_action());
                                ctx.move_images_to(ids, ch.id);
                            } else {
                                ctx.reload_all();
                                ctx.select_view(ActiveView::Channel(ch.id));
                                ctx.toast(&format!("Created #{}", ch.slug));
                            }
                        }
                        Err(e) => ctx.toast(&format!("Could not create channel: {e}")),
                    }
                }
            },
        );
    }

    fn move_images_to(self: &Rc<Self>, ids: Vec<i64>, target_channel_id: i64) {
        if ids.is_empty() {
            self.toast("No image selected to move");
            return;
        }
        match self.store.borrow_mut().move_images(&ids, target_channel_id) {
            Ok(moved) => {
                let target_slug = moved
                    .first()
                    .map(|r| r.channel_slug.clone())
                    .unwrap_or_default();
                if moved.is_empty() {
                    self.toast("Selected image(s) are already in that channel");
                } else {
                    self.select_mode.set(false);
                    self.select_toggle.set_active(false);
                    self.content_stack.set_visible_child_name("gallery");
                    self.toast(&format!("Moved {} image(s) to #{target_slug}", moved.len()));
                    self.reload_all();
                }
            }
            Err(e) => self.toast(&format!("Move failed: {e}")),
        }
    }

    fn prompt_create_channel(self: &Rc<Self>) {
        let entry = gtk4::Entry::builder()
            .placeholder_text("channel name (e.g. frieren)")
            .activates_default(true)
            .build();
        let alert = adw::AlertDialog::builder()
            .heading("New Channel")
            .body("Names are lowercased into clean channel slugs.")
            .extra_child(&entry)
            .build();
        alert.add_response("cancel", "Cancel");
        alert.add_response("create", "Create");
        alert.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("create"));
        alert.set_close_response("cancel");

        let ctx = self.clone();
        let entry_read = entry.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |resp| {
            if resp == "create" {
                let raw = entry_read.text().to_string();
                match ctx.store.borrow().create_channel(&raw) {
                    Ok(ch) => {
                        ctx.reload_all();
                        ctx.select_view(ActiveView::Channel(ch.id));
                        ctx.toast(&format!("Created #{}", ch.slug));
                    }
                    Err(e) => ctx.toast(&format!("Create failed: {e}")),
                }
            }
        });
        entry.grab_focus();
    }

    fn show_channel_menu(self: &Rc<Self>, anchor: &gtk4::Widget, x: i32, y: i32, ch: Channel) {
        let popover = gtk4::Popover::new();
        popover.set_parent(anchor);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));

        let box_menu = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        box_menu.set_margin_top(6);
        box_menu.set_margin_bottom(6);
        box_menu.set_margin_start(6);
        box_menu.set_margin_end(6);

        let rename_btn = gtk4::Button::builder()
            .label("Rename Channel")
            .css_classes(["flat"])
            .build();
        let merge_btn = gtk4::Button::builder()
            .label("Merge Into...")
            .css_classes(["flat"])
            .build();
        let delete_btn = gtk4::Button::builder()
            .label("Delete Channel")
            .css_classes(["flat", "destructive-action"])
            .build();

        box_menu.append(&rename_btn);
        box_menu.append(&merge_btn);
        box_menu.append(&delete_btn);
        popover.set_child(Some(&box_menu));

        let pop_r = popover.clone();
        let ctx_r = self.clone();
        let ch_r = ch.clone();
        rename_btn.connect_clicked(move |_| {
            pop_r.popdown();
            ctx_r.prompt_rename_channel(ch_r.clone());
        });

        let pop_m = popover.clone();
        let ctx_m = self.clone();
        let ch_m = ch.clone();
        merge_btn.connect_clicked(move |_| {
            pop_m.popdown();
            ctx_m.prompt_merge_channel(ch_m.clone());
        });

        let pop_d = popover.clone();
        let ctx_d = self.clone();
        let ch_d = ch.clone();
        delete_btn.connect_clicked(move |_| {
            pop_d.popdown();
            ctx_d.prompt_delete_channel(ch_d.clone());
        });

        popover.connect_closed(|p| p.unparent());
        popover.popup();
    }

    fn prompt_rename_channel(self: &Rc<Self>, ch: Channel) {
        let entry = gtk4::Entry::builder()
            .text(&ch.slug)
            .activates_default(true)
            .build();
        let alert = adw::AlertDialog::builder()
            .heading(format!("Rename #{}", ch.slug))
            .extra_child(&entry)
            .build();
        alert.add_response("cancel", "Cancel");
        alert.add_response("rename", "Rename");
        alert.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("rename"));
        alert.set_close_response("cancel");

        let ctx = self.clone();
        let entry_read = entry.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |resp| {
            if resp == "rename" {
                let raw = entry_read.text().to_string();
                match ctx.store.borrow_mut().rename_channel(ch.id, &raw) {
                    Ok(updated) => {
                        ctx.toast(&format!("Renamed #{} to #{}", ch.slug, updated.slug));
                        ctx.reload_all();
                    }
                    Err(e) => ctx.toast(&format!("Rename failed: {e}")),
                }
            }
        });
        entry.grab_focus();
    }

    fn prompt_merge_channel(self: &Rc<Self>, source: Channel) {
        let others: Vec<Channel> = self
            .channels
            .borrow()
            .iter()
            .filter(|c| c.id != source.id)
            .cloned()
            .collect();
        if others.is_empty() {
            self.toast("Need another channel to merge into");
            return;
        }
        let root_dir = self.store.borrow().root().to_path_buf();
        let ctx = self.clone();
        present_switcher(
            &self.window,
            root_dir,
            self.thumb_cache.clone(),
            others,
            SwitcherMode::MoveSelected,
            move |action| {
                if let SwitcherAction::MoveToChannel(target_id) = action {
                    match ctx.store.borrow_mut().merge_channels(source.id, target_id) {
                        Ok(moved) => {
                            ctx.toast(&format!("Merged #{} ({moved} images)", source.slug));
                            ctx.active_view.set(ActiveView::Channel(target_id));
                            ctx.reload_all();
                        }
                        Err(e) => ctx.toast(&format!("Merge failed: {e}")),
                    }
                }
            },
        );
    }

    fn prompt_delete_channel(self: &Rc<Self>, ch: Channel) {
        let alert = adw::AlertDialog::builder()
            .heading(format!("Delete #{}?", ch.slug))
            .body(format!(
                "This permanently deletes #{} and its {} image(s) from koma.",
                ch.slug, ch.image_count
            ))
            .build();
        alert.add_response("cancel", "Cancel");
        alert.add_response("delete", "Delete");
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");

        let ctx = self.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |resp| {
            if resp == "delete" {
                match ctx.store.borrow_mut().delete_channel(ch.id) {
                    Ok(()) => {
                        ctx.toast(&format!("Deleted #{}", ch.slug));
                        ctx.reload_all();
                    }
                    Err(e) => ctx.toast(&format!("Delete failed: {e}")),
                }
            }
        });
    }

    fn open_lightbox_at(&self, idx: usize) {
        let Some(canvas) = self.grid_canvas.borrow().as_ref().cloned() else {
            return;
        };
        let recs = canvas.records_snapshot();
        if recs.is_empty() {
            return;
        }
        if let Some(lb) = self.lightbox.borrow().as_ref() {
            lb.open(recs, idx);
            self.content_stack.set_visible_child_name("lightbox");
        }
    }
}

pub fn build_ui(app: &adw::Application) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    let theme_monitor = if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        watch_user_gtk_css(&display)
    } else {
        None
    };

    let store = default_data_dir()
        .and_then(Store::open)
        .expect("failed to open koma store");
    let root_dir = store.root().to_path_buf();
    let initial_channels = store.list_channels().expect("failed to list channels");
    let initial_view = initial_channels
        .first()
        .map(|c| ActiveView::Channel(c.id))
        .unwrap_or(ActiveView::Home);
    let initial_mode = match initial_view {
        ActiveView::Home => "timeline",
        ActiveView::Channel(_) => "grid",
    };

    let store = Rc::new(RefCell::new(store));
    let thumb_cache = Rc::new(RefCell::new(ThumbCache::new()));
    let thumb_loader = AsyncThumbLoader::new(thumb_cache.clone());

    // === LEFT SIDEBAR (fixed 214px, collapses on narrow niri splits) ===
    let sidebar = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    sidebar.add_css_class("koma-sidebar");
    sidebar.set_hexpand(false);

    let sidebar_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    sidebar_header.set_margin_top(13);
    sidebar_header.set_margin_bottom(11);
    sidebar_header.set_margin_start(14);
    sidebar_header.set_margin_end(12);

    let wordmark = gtk4::Label::builder()
        .label("koma")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["koma-wordmark"])
        .build();

    let sync_pill = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
    sync_pill.set_valign(gtk4::Align::Center);
    let sync_dot = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    sync_dot.add_css_class("koma-sync-dot");
    sync_dot.set_valign(gtk4::Align::Center);
    let sync_label = gtk4::Label::builder()
        .label("Local")
        .css_classes(["koma-status-inline"])
        .build();
    sync_pill.append(&sync_dot);
    sync_pill.append(&sync_label);

    sidebar_header.append(&wordmark);
    sidebar_header.append(&sync_pill);

    let sidebar_handle = gtk4::WindowHandle::new();
    sidebar_handle.set_child(Some(&sidebar_header));
    sidebar.append(&sidebar_handle);

    let jump_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 7);
    let jump_icon = gtk4::Image::from_icon_name("system-search-symbolic");
    jump_icon.set_opacity(0.45);
    jump_icon.set_pixel_size(13);
    let jump_text = gtk4::Label::builder()
        .label("Jump to...")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["dim-label"])
        .build();
    let jump_kbd = gtk4::Label::builder()
        .label("Ctrl+K")
        .css_classes(["koma-kbd"])
        .build();
    jump_content.append(&jump_icon);
    jump_content.append(&jump_text);
    jump_content.append(&jump_kbd);

    let jump_btn = gtk4::Button::builder()
        .child(&jump_content)
        .margin_start(10)
        .margin_end(10)
        .margin_bottom(8)
        .css_classes(["flat", "koma-jump-btn"])
        .build();
    sidebar.append(&jump_btn);

    let home_inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 7);
    let home_icon = gtk4::Image::from_icon_name("go-home-symbolic");
    home_icon.set_pixel_size(14);
    home_inner.append(&home_icon);
    home_inner.append(
        &gtk4::Label::builder()
            .label("Home")
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    let home_row_btn = gtk4::Button::builder()
        .child(&home_inner)
        .margin_start(8)
        .margin_end(8)
        .margin_bottom(10)
        .css_classes(["flat", "koma-home-btn"])
        .build();
    sidebar.append(&home_row_btn);

    let ch_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    ch_header.set_margin_start(14);
    ch_header.set_margin_end(8);
    ch_header.set_margin_bottom(3);
    let ch_label = gtk4::Label::builder()
        .label("CHANNELS")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["koma-section-label"])
        .build();
    let new_ch_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .css_classes(["flat", "koma-ghost-btn"])
        .tooltip_text("New channel")
        .build();
    ch_header.append(&ch_label);
    ch_header.append(&new_ch_btn);
    sidebar.append(&ch_header);

    let channel_list = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .css_classes(["koma-sidebar-list"])
        .build();
    let ch_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&channel_list)
        .build();
    sidebar.append(&ch_scroll);

    let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    footer.add_css_class("koma-sidebar-footer");
    let footer_count = gtk4::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["koma-tabular"])
        .build();
    let settings_btn = gtk4::Button::builder()
        .icon_name("emblem-system-symbolic")
        .css_classes(["flat", "koma-ghost-btn"])
        .tooltip_text("Storage & shortcuts")
        .build();
    footer.append(&footer_count);
    footer.append(&settings_btn);
    sidebar.append(&footer);

    // === COMPACT SINGLE-LINE TOPBAR ===
    let topbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    topbar.add_css_class("koma-topbar");

    let sidebar_toggle_btn = gtk4::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Toggle Sidebar")
        .active(true)
        .visible(false)
        .css_classes(["flat", "koma-ghost-btn"])
        .build();

    let title_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
    title_row.set_hexpand(true);
    title_row.set_valign(gtk4::Align::Center);
    let title_hash = gtk4::Label::builder()
        .label("#")
        .css_classes(["koma-channel-title", "koma-channel-hash"])
        .build();
    let title_name = gtk4::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .css_classes(["koma-channel-title"])
        .build();
    let title_sub = gtk4::Label::builder()
        .xalign(0.0)
        .valign(gtk4::Align::Baseline)
        .css_classes(["koma-title-count"])
        .build();
    title_row.append(&title_hash);
    title_row.append(&title_name);
    title_row.append(&title_sub);

    let mode_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
    mode_box.add_css_class("koma-segmented");
    mode_box.set_valign(gtk4::Align::Center);
    let grid_toggle = gtk4::ToggleButton::builder()
        .icon_name("view-grid-symbolic")
        .tooltip_text("Gallery view")
        .active(initial_mode == "grid")
        .build();
    let timeline_toggle = gtk4::ToggleButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Timeline view")
        .group(&grid_toggle)
        .active(initial_mode == "timeline")
        .build();
    mode_box.append(&grid_toggle);
    mode_box.append(&timeline_toggle);

    let vsep1 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vsep1.add_css_class("koma-vsep");
    vsep1.set_valign(gtk4::Align::Center);

    let size_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 1.0, 7.0, 1.0);
    size_scale.set_value(4.0);
    size_scale.set_round_digits(0);
    size_scale.set_draw_value(false);
    size_scale.set_size_request(76, -1);
    size_scale.set_valign(gtk4::Align::Center);
    size_scale.add_css_class("koma-slider");
    size_scale.set_tooltip_text(Some("Thumbnail zoom"));

    let vsep2 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vsep2.add_css_class("koma-vsep");
    vsep2.set_valign(gtk4::Align::Center);

    let upload_btn = gtk4::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Upload images (or Ctrl+V / drag-and-drop anywhere)")
        .css_classes(["flat", "koma-ghost-btn"])
        .build();

    let select_toggle = gtk4::ToggleButton::builder()
        .label("Select")
        .css_classes(["flat", "koma-ghost-text"])
        .build();

    let inspector_toggle = gtk4::ToggleButton::builder()
        .icon_name("sidebar-show-right-symbolic")
        .tooltip_text("Toggle Inspector (or double-click any image)")
        .active(false)
        .css_classes(["flat", "koma-ghost-btn"])
        .build();

    topbar.append(&sidebar_toggle_btn);
    topbar.append(&title_row);
    topbar.append(&mode_box);
    topbar.append(&vsep1);
    topbar.append(&size_scale);
    topbar.append(&vsep2);
    topbar.append(&upload_btn);
    topbar.append(&select_toggle);
    topbar.append(&inspector_toggle);

    let topbar_handle = gtk4::WindowHandle::new();
    topbar_handle.set_child(Some(&topbar));

    // === MULTI-SELECT FLOATING BAR (ONLY VISIBLE WHEN MULTI-SELECTING) ===
    let batch_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    batch_bar.add_css_class("koma-floating-dock");
    batch_bar.set_halign(gtk4::Align::Center);
    batch_bar.set_valign(gtk4::Align::End);
    batch_bar.set_visible(false);

    let batch_count_label = gtk4::Label::builder()
        .css_classes(["koma-inspector-filename"])
        .margin_end(4)
        .build();
    let batch_move_btn = gtk4::Button::builder()
        .label("Move")
        .css_classes(["flat", "koma-ghost-text"])
        .build();
    let batch_copy_btn = gtk4::Button::builder()
        .label("Copy")
        .css_classes(["flat", "koma-ghost-text"])
        .build();
    let batch_zip_btn = gtk4::Button::builder()
        .label("Export Zip")
        .css_classes(["flat", "koma-accent-btn"])
        .build();
    let batch_del_btn = gtk4::Button::builder()
        .label("Delete")
        .css_classes(["flat", "koma-ghost-text", "destructive-action"])
        .build();
    let batch_esc_btn = gtk4::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Clear selection (Esc)")
        .css_classes(["flat", "koma-ghost-btn"])
        .build();
    batch_bar.append(&batch_count_label);
    batch_bar.append(&batch_move_btn);
    batch_bar.append(&batch_copy_btn);
    batch_bar.append(&batch_zip_btn);
    batch_bar.append(&batch_del_btn);
    batch_bar.append(&batch_esc_btn);

    // === MAIN LAYOUT ASSEMBLY ===
    // 0px-height width sensor triggers justified row re-packing on resize or Inspector toggle.
    let width_sensor = gtk4::DrawingArea::new();
    width_sensor.set_content_height(0);
    width_sensor.set_hexpand(true);

    let canvas_overlay = gtk4::Overlay::new();
    canvas_overlay.set_vexpand(true);
    canvas_overlay.set_hexpand(true);
    canvas_overlay.add_overlay(&batch_bar);

    let canvas_col = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    canvas_col.add_css_class("koma-canvas-bg");
    canvas_col.set_hexpand(true);
    canvas_col.set_vexpand(true);
    canvas_col.append(&width_sensor);
    canvas_col.append(&canvas_overlay);

    let inspector_split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk4::PackType::End)
        .pin_sidebar(true)
        .show_sidebar(false)
        .min_sidebar_width(248.0)
        .max_sidebar_width(276.0)
        .hexpand(true)
        .vexpand(true)
        .content(&canvas_col)
        .build();

    let gallery_pane = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    gallery_pane.set_hexpand(true);
    gallery_pane.set_vexpand(true);
    gallery_pane.append(&topbar_handle);
    gallery_pane.append(&inspector_split);

    // Lightbox lives inside the right pane so the left channel sidebar remains visible.
    let content_stack = gtk4::Stack::new();
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);
    content_stack.add_named(&gallery_pane, Some("gallery"));

    let left_split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk4::PackType::Start)
        .pin_sidebar(true)
        .show_sidebar(true)
        .min_sidebar_width(214.0)
        .max_sidebar_width(214.0)
        .sidebar_width_fraction(0.0)
        .sidebar(&sidebar)
        .content(&content_stack)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&left_split));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("koma")
        .default_width(1260)
        .default_height(800)
        .content(&toast_overlay)
        .build();

    // Responsive breakpoints for narrow niri splits.
    let bp_narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        760.0,
        adw::LengthUnit::Px,
    ));
    bp_narrow.add_setter(&left_split, "collapsed", Some(&true.to_value()));
    bp_narrow.add_setter(&inspector_split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp_narrow);

    let ctx = Rc::new(AppCtx {
        window: window.clone(),
        toast_overlay,
        content_stack: content_stack.clone(),
        store: store.clone(),
        thumb_cache: thumb_cache.clone(),
        channels: RefCell::new(initial_channels),
        active_view: Cell::new(initial_view),
        view_mode: RefCell::new(initial_mode.to_string()),
        selected_images: RefCell::new(Vec::new()),
        focused_image: RefCell::new(None),
        select_mode: Cell::new(false),
        suppress_sidebar_select: Cell::new(false),
        left_split: left_split.clone(),
        sidebar_toggle_btn: sidebar_toggle_btn.clone(),
        home_row_btn: home_row_btn.clone(),
        channel_list: channel_list.clone(),
        footer_count,
        title_hash,
        title_name,
        title_sub,
        upload_btn: upload_btn.clone(),
        grid_toggle: grid_toggle.clone(),
        timeline_toggle: timeline_toggle.clone(),
        select_toggle: select_toggle.clone(),
        inspector_split: inspector_split.clone(),
        inspector_toggle: inspector_toggle.clone(),
        batch_bar,
        batch_count_label,
        _theme_monitor: theme_monitor,
        grid_canvas: RefCell::new(None),
        inspector: RefCell::new(None),
        lightbox: RefCell::new(None),
    });

    // Build GridCanvas (double-click toggles Inspector on same image)
    let grid_canvas = GridCanvas::new(
        root_dir.clone(),
        thumb_loader,
        {
            let ctx = ctx.clone();
            move |selected, focused| {
                *ctx.selected_images.borrow_mut() = selected;
                *ctx.focused_image.borrow_mut() = focused.clone();
                if ctx.inspector_split.shows_sidebar()
                    && let Some(ins) = ctx.inspector.borrow().as_ref()
                {
                    ins.set_image(focused, &ctx.channels.borrow());
                }
                ctx.update_batch_bar();
            }
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.toggle_inspector_for(rec)
        },
        {
            let ctx = ctx.clone();
            move |cid| ctx.select_view(ActiveView::Channel(cid))
        },
    );
    canvas_overlay.set_child(Some(&grid_canvas.stack));
    *ctx.grid_canvas.borrow_mut() = Some(grid_canvas.clone());

    let gc_resize = grid_canvas.clone();
    width_sensor.connect_resize(move |_, width, _| {
        if width > 0 {
            gc_resize.set_viewport_width(width);
        }
    });

    // Build Inspector
    let inspector_pane = InspectorPane::new(
        root_dir.clone(),
        thumb_cache.clone(),
        {
            let ctx = ctx.clone();
            move || {
                ctx.inspector_split.set_show_sidebar(false);
                ctx.inspector_toggle.set_active(false);
            }
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.copy_image_to_clipboard(&rec)
        },
        {
            let ctx = ctx.clone();
            move |rec| {
                let idx = ctx
                    .grid_canvas
                    .borrow()
                    .as_ref()
                    .and_then(|g| g.index_of(rec.id))
                    .unwrap_or(0);
                ctx.open_lightbox_at(idx);
            }
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.open_switcher(SwitcherMode::MoveSelected, Some(vec![rec.id]))
        },
        {
            let ctx = ctx.clone();
            move |id, new_name| match ctx.store.borrow_mut().rename_image(id, &new_name) {
                Ok(updated) => {
                    ctx.toast(&format!("Renamed to {}", updated.filename));
                    ctx.reload_images();
                }
                Err(e) => ctx.toast(&format!("Rename failed: {e}")),
            }
        },
        {
            let ctx = ctx.clone();
            move |image_id, target_cid| ctx.move_images_to(vec![image_id], target_cid)
        },
    );
    inspector_split.set_sidebar(Some(&inspector_pane.root_widget));
    *ctx.inspector.borrow_mut() = Some(inspector_pane);

    // Build Lightbox inside right content_stack so sidebar stays open.
    let lightbox_view = LightboxView::new(
        root_dir,
        thumb_cache,
        {
            let ctx = ctx.clone();
            move || {
                ctx.content_stack.set_visible_child_name("gallery");
                if let Some(canvas) = ctx.grid_canvas.borrow().as_ref() {
                    canvas.focus_canvas();
                }
            }
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.copy_image_to_clipboard(&rec)
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.open_switcher(SwitcherMode::MoveSelected, Some(vec![rec.id]))
        },
    );
    content_stack.add_named(&lightbox_view.root_widget, Some("lightbox"));
    *ctx.lightbox.borrow_mut() = Some(lightbox_view);

    // === SIGNAL WIRING ===
    let left_ref = left_split.clone();
    sidebar_toggle_btn.connect_toggled(move |btn| {
        left_ref.set_show_sidebar(btn.is_active());
    });
    let st_btn = sidebar_toggle_btn.clone();
    left_split.connect_collapsed_notify(move |split| {
        st_btn.set_visible(split.is_collapsed() || !split.shows_sidebar());
    });
    let st_btn2 = sidebar_toggle_btn.clone();
    left_split.connect_show_sidebar_notify(move |split| {
        let showing = split.shows_sidebar();
        if st_btn2.is_active() != showing {
            st_btn2.set_active(showing);
        }
        st_btn2.set_visible(split.is_collapsed() || !showing);
    });

    let c = ctx.clone();
    jump_btn.connect_clicked(move |_| c.open_switcher(SwitcherMode::Jump, None));

    let c = ctx.clone();
    home_row_btn.connect_clicked(move |_| c.select_view(ActiveView::Home));

    let c = ctx.clone();
    new_ch_btn.connect_clicked(move |_| c.prompt_create_channel());

    let c = ctx.clone();
    channel_list.connect_row_selected(move |_, row| {
        if c.suppress_sidebar_select.get() {
            return;
        }
        if let Some(row) = row
            && let Ok(id) = row.widget_name().parse::<i64>()
        {
            c.select_view(ActiveView::Channel(id));
        }
    });

    let c = ctx.clone();
    settings_btn.connect_clicked(move |_| {
        let dir = c.store.borrow().root().display().to_string();
        let alert = adw::AlertDialog::builder()
            .heading("koma Storage")
            .body(format!(
                "Data folder: {dir}\nSync backend: Local only (Google Drive sync comes in Step 4).\n\nShortcuts: Ctrl+V paste, Ctrl+K switch channel, Ctrl+A select all, Space open full lightbox, double-click toggle Inspector.\nRight-click any channel in the sidebar to rename, merge, or delete it."
            ))
            .build();
        alert.add_response("ok", "OK");
        alert.set_default_response(Some("ok"));
        alert.present(Some(&c.window));
    });

    let c = ctx.clone();
    grid_toggle.connect_toggled(move |btn| {
        if btn.is_active() {
            *c.view_mode.borrow_mut() = "grid".to_string();
            if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
                canvas.set_mode("grid");
                canvas.focus_canvas();
            }
        }
    });

    let c = ctx.clone();
    timeline_toggle.connect_toggled(move |btn| {
        if btn.is_active() {
            *c.view_mode.borrow_mut() = "timeline".to_string();
            if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
                canvas.set_mode("timeline");
                canvas.focus_canvas();
            }
        }
    });

    let c = ctx.clone();
    size_scale.connect_value_changed(move |scale| {
        if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
            canvas.set_zoom_level(scale.value().round() as i32);
        }
    });

    let c = ctx.clone();
    select_toggle.connect_toggled(move |btn| {
        let active = btn.is_active();
        c.select_mode.set(active);
        if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
            canvas.set_select_mode(active);
            if !active {
                canvas.clear_selection();
            }
        }
        c.update_batch_bar();
    });

    let c_ins = ctx.clone();
    inspector_toggle.connect_toggled(move |btn| {
        let show = btn.is_active();
        if show
            && let Some(ins) = c_ins.inspector.borrow().as_ref()
        {
            ins.set_image(c_ins.focused_image.borrow().clone(), &c_ins.channels.borrow());
        }
        c_ins.inspector_split.set_show_sidebar(show);
    });
    let ins_btn = inspector_toggle.clone();
    inspector_split.connect_show_sidebar_notify(move |split| {
        let showing = split.shows_sidebar();
        if ins_btn.is_active() != showing {
            ins_btn.set_active(showing);
        }
    });

    let c = ctx.clone();
    upload_btn.connect_clicked(move |_| c.open_upload_dialog());

    let c = ctx.clone();
    batch_move_btn.connect_clicked(move |_| c.open_switcher(SwitcherMode::MoveSelected, None));

    let c = ctx.clone();
    batch_copy_btn.connect_clicked(move |_| {
        if let Some(rec) = c
            .selected_images
            .borrow()
            .first()
            .cloned()
            .or_else(|| c.focused_image.borrow().clone())
        {
            c.copy_image_to_clipboard(&rec);
        }
    });

    let c = ctx.clone();
    batch_zip_btn.connect_clicked(move |_| c.export_selected_zip());

    let c = ctx.clone();
    batch_del_btn.connect_clicked(move |_| c.confirm_delete_selected());

    let c = ctx.clone();
    batch_esc_btn.connect_clicked(move |_| {
        c.select_mode.set(false);
        c.select_toggle.set_active(false);
        if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
            canvas.clear_selection();
        }
        c.update_batch_bar();
    });

    // Global keyboard shortcuts (Ctrl+K, Ctrl+V, Ctrl+A, Space, Esc, Lightbox arrows)
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let c = ctx.clone();
    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        if c.content_stack.visible_child_name().as_deref() == Some("lightbox") {
            match key {
                gdk::Key::Escape => {
                    c.content_stack.set_visible_child_name("gallery");
                    return glib::Propagation::Stop;
                }
                gdk::Key::Left => {
                    if let Some(lb) = c.lightbox.borrow().as_ref() {
                        lb.step(-1);
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Right => {
                    if let Some(lb) = c.lightbox.borrow().as_ref() {
                        lb.step(1);
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::z | gdk::Key::Z => {
                    if let Some(lb) = c.lightbox.borrow().as_ref() {
                        lb.toggle_zoom();
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        let editing_text = gtk4::prelude::GtkWindowExt::focus(&c.window)
            .is_some_and(|w| w.is::<gtk4::Editable>() || w.is::<gtk4::Text>());

        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(key, gdk::Key::k | gdk::Key::K) {
            c.open_switcher(SwitcherMode::Jump, None);
            return glib::Propagation::Stop;
        }
        if !editing_text && ctrl && matches!(key, gdk::Key::v | gdk::Key::V) {
            c.paste_from_clipboard();
            return glib::Propagation::Stop;
        }
        if !editing_text && ctrl && matches!(key, gdk::Key::a | gdk::Key::A) {
            if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
                canvas.select_all();
            }
            return glib::Propagation::Stop;
        }
        if !editing_text && key == gdk::Key::space {
            if let Some(rec) = c.focused_image.borrow().clone() {
                let idx = c
                    .grid_canvas
                    .borrow()
                    .as_ref()
                    .and_then(|g| g.index_of(rec.id))
                    .unwrap_or(0);
                c.open_lightbox_at(idx);
                return glib::Propagation::Stop;
            }
        }
        if key == gdk::Key::Escape {
            if c.select_mode.get() || c.selected_images.borrow().len() > 1 {
                c.select_mode.set(false);
                c.select_toggle.set_active(false);
                if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
                    canvas.clear_selection();
                }
                c.update_batch_bar();
                return glib::Propagation::Stop;
            } else if c.inspector_split.shows_sidebar() {
                c.inspector_split.set_show_sidebar(false);
                c.inspector_toggle.set_active(false);
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    window.add_controller(key_ctrl);

    // Whole-window file drag-and-drop
    let drop_target = gtk4::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    let c = ctx.clone();
    drop_target.connect_drop(move |_, value, _, _| {
        if let Ok(file_list) = value.get::<gdk::FileList>() {
            let paths: Vec<PathBuf> = file_list
                .files()
                .into_iter()
                .filter_map(|f| f.path())
                .collect();
            if !paths.is_empty() {
                c.ingest_paths(&paths);
                return true;
            }
        }
        false
    });
    window.add_controller(drop_target);

    ctx.reload_all();
    window.present();

    // Wayland/niri activates the surface asynchronously after present(), so defer canvas focus.
    let ctx_idle = ctx.clone();
    glib::idle_add_local_once(move || {
        if let Some(canvas) = ctx_idle.grid_canvas.borrow().as_ref() {
            gtk4::prelude::GtkWindowExt::set_focus(
                &ctx_idle.window,
                Some(&canvas.focus_widget()),
            );
        }
    });
    let focused_once = Rc::new(Cell::new(false));
    let ctx_act = ctx.clone();
    window.connect_is_active_notify(move |win| {
        if win.is_active()
            && !focused_once.replace(true)
            && let Some(canvas) = ctx_act.grid_canvas.borrow().as_ref()
        {
            gtk4::prelude::GtkWindowExt::set_focus(win, Some(&canvas.focus_widget()));
        }
    });
}

fn watch_user_gtk_css(display: &gdk::Display) -> Option<gio::FileMonitor> {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let gtk_css = config_dir.join("gtk-4.0/gtk.css");
    let file = gio::File::for_path(&gtk_css);

    // Reload ~/.config/gtk-4.0/gtk.css live whenever iNiR/matugen overwrites it on wallpaper change.
    let user_provider = gtk4::CssProvider::new();
    if gtk_css.is_file() {
        user_provider.load_from_path(&gtk_css);
    }
    gtk4::style_context_add_provider_for_display(
        display,
        &user_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_USER,
    );

    let monitor = file
        .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    let path_for_cb = gtk_css;
    monitor.connect_changed(move |_, _, _, event| {
        if matches!(
            event,
            gio::FileMonitorEvent::Changed
                | gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::MovedIn
        ) && path_for_cb.is_file()
        {
            user_provider.load_from_path(&path_for_cb);
        }
    });
    Some(monitor)
}

fn format_int(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
