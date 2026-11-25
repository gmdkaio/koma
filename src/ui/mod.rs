mod grid;
mod inspector;
mod lightbox;
mod media_util;
mod switcher;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::storage::gdrive::{self, GDriveClientConfig};
use crate::storage::sync::{SyncEvent, SyncHandle, SyncStatus};
use crate::store::{Channel, ChannelSort, ImageRecord, ImageSort, Store, default_data_dir};
use crate::ui::grid::{GridCanvas, NavDir};
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
    sync: SyncHandle,
    sync_status: RefCell<SyncStatus>,
    thumb_cache: Rc<RefCell<ThumbCache>>,
    channels: RefCell<Vec<Channel>>,
    channel_sort: Cell<ChannelSort>,
    image_sort: Cell<ImageSort>,
    active_view: Cell<ActiveView>,
    view_mode: RefCell<String>,
    selected_images: RefCell<Vec<ImageRecord>>,
    focused_image: RefCell<Option<ImageRecord>>,
    select_mode: Cell<bool>,
    suppress_sidebar_select: Cell<bool>,
    suppress_inspector_toggle: Cell<bool>,

    // Sidebar widgets
    left_split: adw::OverlaySplitView,
    sidebar_toggle_btn: gtk4::ToggleButton,
    sync_pill: gtk4::Box,
    sync_dot: gtk4::Box,
    sync_label: gtk4::Label,
    home_row_btn: gtk4::Button,
    new_ch_revealer: gtk4::Revealer,
    new_ch_entry: gtk4::Entry,
    editing_channel_id: Cell<Option<i64>>,
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

    // Multi-select floating bar (hidden unless multi-selecting)
    batch_revealer: gtk4::Revealer,
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

    fn apply_sync_status(&self, status: SyncStatus) {
        self.sync_dot.remove_css_class("sync-ok");
        self.sync_dot.remove_css_class("sync-busy");
        self.sync_dot.remove_css_class("sync-offline");
        match &status {
            SyncStatus::LocalOnly => {
                self.sync_label.set_label("Local");
                self.sync_pill
                    .set_tooltip_text(Some("Local only (connect Google Drive in settings)"));
            }
            SyncStatus::Syncing => {
                self.sync_dot.add_css_class("sync-busy");
                self.sync_label.set_label("Syncing");
                self.sync_pill
                    .set_tooltip_text(Some("Syncing with Google Drive..."));
            }
            SyncStatus::Synced(summary) => {
                self.sync_dot.add_css_class("sync-ok");
                self.sync_label.set_label("Synced");
                self.sync_pill.set_tooltip_text(Some(summary));
            }
            SyncStatus::Offline(err) => {
                self.sync_dot.add_css_class("sync-offline");
                self.sync_label.set_label("Offline");
                self.sync_pill
                    .set_tooltip_text(Some(&format!("Sync paused: {err}")));
            }
        }
        *self.sync_status.borrow_mut() = status;
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
        let channels = match self
            .store
            .borrow()
            .list_channels_sorted(self.channel_sort.get())
        {
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
            self.active_view.set(ActiveView::Home);
        }

        *self.channels.borrow_mut() = channels;
        self.rebuild_sidebar();
        self.reload_images_with_scroll(true);
    }

    fn reorder_channel_drop(self: &Rc<Self>, dragged_id: i64, target_id: i64) {
        if dragged_id == target_id {
            return;
        }
        let mut ids: Vec<i64> = self.channels.borrow().iter().map(|c| c.id).collect();
        let Some(from) = ids.iter().position(|&id| id == dragged_id) else {
            return;
        };
        let Some(to) = ids.iter().position(|&id| id == target_id) else {
            return;
        };
        let item = ids.remove(from);
        ids.insert(to, item);

        if let Err(e) = self.store.borrow_mut().save_channel_order(&ids) {
            self.toast(&format!("Reorder failed: {e}"));
            return;
        }
        self.channel_sort.set(ChannelSort::Custom);
        let _ = self
            .store
            .borrow()
            .set_setting("channel_sort", ChannelSort::Custom.as_str());
        self.reload_all();
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

        let editing_id = self.editing_channel_id.get();
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
            row_box.append(&hash);

            if editing_id == Some(ch.id) {
                let entry = gtk4::Entry::builder()
                    .text(&ch.slug)
                    .hexpand(true)
                    .css_classes(["koma-inline-entry"])
                    .build();
                let ctx_act = self.clone();
                let ch_id = ch.id;
                let old_slug = ch.slug.clone();
                entry.connect_activate(move |e| {
                    let raw = e.text().to_string();
                    ctx_act.editing_channel_id.set(None);
                    if raw.trim().is_empty() || raw.trim() == old_slug {
                        ctx_act.rebuild_sidebar();
                        return;
                    }
                    match ctx_act.store.borrow_mut().rename_channel(ch_id, &raw) {
                        Ok(updated) => {
                            ctx_act.toast(&format!("Renamed #{old_slug} to #{}", updated.slug));
                            ctx_act.reload_all();
                            ctx_act.sync.trigger_sync();
                        }
                        Err(err) => {
                            ctx_act.toast(&format!("Rename failed: {err}"));
                            ctx_act.rebuild_sidebar();
                        }
                    }
                });
                let key_ctrl = gtk4::EventControllerKey::new();
                let ctx_esc = self.clone();
                key_ctrl.connect_key_pressed(move |_, key, _, _| {
                    if key == gdk::Key::Escape {
                        ctx_esc.editing_channel_id.set(None);
                        ctx_esc.rebuild_sidebar();
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                });
                entry.add_controller(key_ctrl);

                let focus_ctrl = gtk4::EventControllerFocus::new();
                let ctx_blur = self.clone();
                focus_ctrl.connect_leave(move |_| {
                    if ctx_blur.editing_channel_id.get() == Some(ch_id) {
                        ctx_blur.editing_channel_id.set(None);
                        let c2 = ctx_blur.clone();
                        glib::idle_add_local_once(move || c2.rebuild_sidebar());
                    }
                });
                entry.add_controller(focus_ctrl);

                row_box.append(&entry);
                let e_focus = entry.clone();
                glib::idle_add_local_once(move || {
                    e_focus.grab_focus();
                    e_focus.select_region(0, -1);
                });
            } else {
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
                row_box.append(&name);
                row_box.append(&count);
            }

            let row = gtk4::ListBoxRow::builder().child(&row_box).build();
            row.set_widget_name(&ch.id.to_string());

            // Drag-and-drop channel reordering
            let drag = gtk4::DragSource::builder()
                .actions(gdk::DragAction::MOVE)
                .build();
            let drag_payload = format!("koma-ch:{}", ch.id);
            drag.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&drag_payload.to_value()))
            });
            row.add_controller(drag);

            let drop = gtk4::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
            let row_enter = row.clone();
            drop.connect_enter(move |_, _, _| {
                row_enter.add_css_class("koma-row-drop-target");
                gdk::DragAction::MOVE
            });
            let row_leave = row.clone();
            drop.connect_leave(move |_| {
                row_leave.remove_css_class("koma-row-drop-target");
            });
            let ctx_drop = self.clone();
            let target_id = ch.id;
            let row_drop = row.clone();
            drop.connect_drop(move |_, val, _, _| {
                row_drop.remove_css_class("koma-row-drop-target");
                if let Ok(text) = val.get::<String>()
                    && let Some(raw_id) = text.strip_prefix("koma-ch:")
                    && let Ok(src_id) = raw_id.parse::<i64>()
                {
                    ctx_drop.reorder_channel_drop(src_id, target_id);
                    return true;
                }
                false
            });
            row.add_controller(drop);

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
        self.reload_images_with_scroll(true);
    }

    fn reload_images_with_scroll(self: &Rc<Self>, preserve_scroll: bool) {
        let active = self.active_view.get();
        let sort = self.image_sort.get();
        let sort_tag = match sort {
            ImageSort::Newest => "",
            ImageSort::Oldest => " · Oldest",
        };
        let (channel_id, title, sub, is_home) = match active {
            ActiveView::Home => {
                let total: i64 = self.channels.borrow().iter().map(|c| c.image_count).sum();
                (
                    None,
                    "Home".to_string(),
                    format!("{}{sort_tag}", format_int(total)),
                    true,
                )
            }
            ActiveView::Channel(id) => {
                let ch = self.channels.borrow().iter().find(|c| c.id == id).cloned();
                match ch {
                    Some(c) => (
                        Some(c.id),
                        c.slug.clone(),
                        format!("{}{sort_tag}", format_int(c.image_count)),
                        false,
                    ),
                    None => (None, "Home".to_string(), format!("Recent{sort_tag}"), true),
                }
            }
        };

        self.title_hash.set_visible(!is_home);
        self.title_name.set_label(&title);
        self.title_sub.set_label(&sub);
        self.upload_btn.set_sensitive(!is_home);

        let records = match self.store.borrow().list_images_sorted(channel_id, sort) {
            Ok(r) => r,
            Err(e) => {
                self.toast(&format!("Failed to load images: {e}"));
                return;
            }
        };

        let in_lightbox = self.content_stack.visible_child_name().as_deref() == Some("lightbox");
        if in_lightbox
            && let Some(lb) = self.lightbox.borrow().as_ref().cloned()
        {
            let cur_id = lb.current_record().map(|r| r.id);
            if !lb.sync_records(records.clone(), cur_id) {
                self.close_lightbox();
            }
        }

        let mode = self.view_mode.borrow().clone();
        if let Some(canvas) = self.grid_canvas.borrow().as_ref() {
            canvas.set_records(
                records,
                &mode,
                is_home,
                sort.is_chronological(),
                preserve_scroll,
            );
            if !in_lightbox {
                gtk4::prelude::GtkWindowExt::set_focus(&self.window, Some(&canvas.focus_widget()));
            }
        }
        self.update_batch_bar();
    }

    fn select_view(self: &Rc<Self>, view: ActiveView) {
        let same_view = self.active_view.get() == view
            && self.content_stack.visible_child_name().as_deref() == Some("gallery");
        self.close_lightbox();
        self.active_view.set(view);
        self.select_mode.set(false);
        self.select_toggle.set_active(false);

        if !same_view {
            let default_mode = match view {
                ActiveView::Home => "timeline",
                ActiveView::Channel(_) => "grid",
            };
            *self.view_mode.borrow_mut() = default_mode.to_string();
            self.grid_toggle.set_active(default_mode == "grid");
            self.timeline_toggle.set_active(default_mode == "timeline");
        }

        if self.left_split.is_collapsed() {
            self.left_split.set_show_sidebar(false);
            self.sidebar_toggle_btn.set_active(false);
        }

        self.rebuild_sidebar();
        self.reload_images_with_scroll(same_view);
    }

    fn step_view(self: &Rc<Self>, delta: i32) {
        let mut views = vec![ActiveView::Home];
        for ch in self.channels.borrow().iter() {
            views.push(ActiveView::Channel(ch.id));
        }
        let cur = self.active_view.get();
        let cur_idx = views.iter().position(|&v| v == cur).unwrap_or(0) as i32;
        let next_idx = (cur_idx + delta).clamp(0, (views.len() as i32) - 1) as usize;
        let next = views[next_idx];
        if next != cur {
            self.select_view(next);
        }
    }

    fn set_inspector_visible(&self, show: bool) {
        if self.inspector_split.shows_sidebar() == show {
            return;
        }
        if !self.inspector_split.is_collapsed()
            && let Some(canvas) = self.grid_canvas.borrow().clone()
        {
            let delta = if show { -276 } else { 276 };
            canvas.notify_sidebar_animating(delta);
        }
        self.inspector_split.set_show_sidebar(show);
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
            self.set_inspector_visible(false);
            return;
        }

        *self.focused_image.borrow_mut() = Some(rec.clone());
        let ins = self.inspector.borrow().clone();
        if let Some(ins) = ins {
            let chs = self.channels.borrow().clone();
            ins.set_image(Some(rec), &chs);
        }
        self.set_inspector_visible(true);
    }

    fn update_batch_bar(&self) {
        let count = self.selected_images.borrow().len();
        let show_batch = count > 1 || (self.select_mode.get() && count >= 1);
        if show_batch {
            self.batch_count_label
                .set_label(&format!("{count} selected"));
        }
        self.batch_revealer.set_reveal_child(show_batch);
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
                        ctx.sync.trigger_sync();
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
            self.sync.trigger_sync();
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
        self.confirm_delete_ids(ids);
    }

    fn confirm_delete_ids(self: &Rc<Self>, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        let heading = if ids.len() == 1 {
            "Delete image?".to_string()
        } else {
            format!("Delete {} images?", ids.len())
        };
        let ctx = self.clone();
        present_confirm_dialog(
            &self.window,
            &heading,
            "Removes the selected file(s) from koma and trashes them in Google Drive on next sync.",
            "Delete",
            move || match ctx.store.borrow_mut().delete_images(&ids) {
                Ok(n) => {
                    ctx.select_mode.set(false);
                    if ctx.select_toggle.is_active() {
                        ctx.select_toggle.set_active(false);
                    }
                    ctx.toast(&format!("Deleted {n} image(s)"));
                    ctx.reload_all();
                    ctx.sync.trigger_sync();
                }
                Err(e) => ctx.toast(&format!("Delete failed: {e}")),
            },
        );
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
                                ctx.sync.trigger_sync();
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
                    self.close_lightbox();
                    self.toast(&format!("Moved {} image(s) to #{target_slug}", moved.len()));
                    self.reload_all();
                    self.sync.trigger_sync();
                }
            }
            Err(e) => self.toast(&format!("Move failed: {e}")),
        }
    }

    fn begin_inline_create_channel(&self) {
        self.new_ch_entry.set_text("");
        self.new_ch_revealer.set_reveal_child(true);
        let entry = self.new_ch_entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });
    }

    fn begin_inline_rename_channel(self: &Rc<Self>, ch_id: i64) {
        self.editing_channel_id.set(Some(ch_id));
        self.rebuild_sidebar();
    }

    fn show_channel_menu(self: &Rc<Self>, anchor: &gtk4::Widget, x: i32, y: i32, ch: Channel) {
        let popover = gtk4::Popover::new();
        popover.add_css_class("koma-settings-pop");
        popover.add_css_class("koma-menu-pop");
        popover.set_has_arrow(false);
        popover.set_parent(anchor);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));

        let box_menu = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        box_menu.set_size_request(156, -1);

        let rename_btn = gtk4::Button::builder()
            .label("Rename  (F2)")
            .css_classes(["flat", "koma-menu-item"])
            .build();
        let merge_btn = gtk4::Button::builder()
            .label("Merge into...")
            .css_classes(["flat", "koma-menu-item"])
            .build();
        let delete_btn = gtk4::Button::builder()
            .label("Delete channel")
            .css_classes(["flat", "koma-menu-item", "destructive-action"])
            .build();

        box_menu.append(&rename_btn);
        box_menu.append(&merge_btn);
        box_menu.append(&delete_btn);
        popover.set_child(Some(&box_menu));

        let pop_r = popover.clone();
        let ctx_r = self.clone();
        let ch_id = ch.id;
        rename_btn.connect_clicked(move |_| {
            pop_r.popdown();
            ctx_r.begin_inline_rename_channel(ch_id);
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
                            ctx.sync.trigger_sync();
                        }
                        Err(e) => ctx.toast(&format!("Merge failed: {e}")),
                    }
                }
            },
        );
    }

    fn prompt_delete_channel(self: &Rc<Self>, ch: Channel) {
        let body = format!(
            "Removes #{} and its {} image(s) from koma.",
            ch.slug, ch.image_count
        );
        let ctx = self.clone();
        present_confirm_dialog(
            &self.window,
            &format!("Delete #{}?", ch.slug),
            &body,
            "Delete",
            move || match ctx.store.borrow_mut().delete_channel(ch.id) {
                Ok(()) => {
                    ctx.toast(&format!("Deleted #{}", ch.slug));
                    ctx.reload_all();
                    ctx.sync.trigger_sync();
                }
                Err(e) => ctx.toast(&format!("Delete failed: {e}")),
            },
        );
    }

    fn close_lightbox(&self) {
        self.content_stack.set_visible_child_name("gallery");
        let ins = self.inspector.borrow().clone();
        if let Some(ins) = ins {
            ins.set_lightbox_mode(false);
        }
    }

    fn open_lightbox_at(&self, idx: usize) {
        let Some(canvas) = self.grid_canvas.borrow().clone() else {
            return;
        };
        let recs = canvas.records_snapshot();
        if recs.is_empty() {
            return;
        }
        let ins = self.inspector.borrow().clone();
        if let Some(ins) = ins {
            ins.set_lightbox_mode(true);
        }
        let lb = self.lightbox.borrow().clone();
        if let Some(lb) = lb {
            lb.set_inspector_active(self.inspector_split.shows_sidebar());
            lb.open(recs, idx);
            self.content_stack.set_visible_child_name("lightbox");
        }
    }
}

pub fn build_ui(app: &adw::Application) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    let theme_monitor = if let Some(display) = gdk::Display::default() {
        let monitor = watch_user_gtk_css(&display);
        // Load koma's stylesheet above USER priority so built-in light/dark themes
        // override dark-only variables from ~/.config/gtk-4.0/gtk.css cleanly.
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
        monitor
    } else {
        None
    };

    let store = default_data_dir()
        .and_then(Store::open)
        .expect("failed to open koma store");
    let root_dir = store.root().to_path_buf();
    let saved_channel_sort = store
        .get_setting("channel_sort")
        .map(|s| ChannelSort::parse(&s))
        .unwrap_or(ChannelSort::Count);
    let saved_image_sort = store
        .get_setting("image_sort")
        .map(|s| ImageSort::parse(&s))
        .unwrap_or(ImageSort::Newest);
    let initial_channels = store
        .list_channels_sorted(saved_channel_sort)
        .expect("failed to list channels");
    let initial_view = ActiveView::Home;
    let initial_mode = "timeline";

    let store = Rc::new(RefCell::new(store));
    let sync = SyncHandle::spawn(root_dir.clone());
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

    let brand_icon = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../../assets/icon.png"
    )))
    .ok()
    .map(|tex| {
        let img = gtk4::Image::from_paintable(Some(&tex));
        img.set_pixel_size(22);
        img.set_valign(gtk4::Align::Center);
        img
    });

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

    if let Some(icon) = brand_icon {
        sidebar_header.append(&icon);
    }
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
    let ch_sort_btn = gtk4::Button::builder()
        .icon_name("view-sort-descending-symbolic")
        .css_classes(["flat", "koma-ghost-btn"])
        .tooltip_text("Sort channels (or drag to reorder)")
        .build();
    let new_ch_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .css_classes(["flat", "koma-ghost-btn"])
        .tooltip_text("New channel")
        .build();
    ch_header.append(&ch_label);
    ch_header.append(&ch_sort_btn);
    ch_header.append(&new_ch_btn);
    sidebar.append(&ch_header);

    // Inline channel creation row (slides down on +, no modal popup)
    let new_ch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
    new_ch_row.set_margin_start(14);
    new_ch_row.set_margin_end(12);
    new_ch_row.set_margin_top(2);
    new_ch_row.set_margin_bottom(6);
    let new_ch_hash = gtk4::Label::builder()
        .label("#")
        .css_classes(["koma-channel-hash"])
        .build();
    let new_ch_entry = gtk4::Entry::builder()
        .placeholder_text("channel-name")
        .hexpand(true)
        .css_classes(["koma-inline-entry"])
        .build();
    new_ch_row.append(&new_ch_hash);
    new_ch_row.append(&new_ch_entry);

    let new_ch_revealer = gtk4::Revealer::builder()
        .transition_type(gtk4::RevealerTransitionType::SlideDown)
        .transition_duration(140)
        .reveal_child(false)
        .child(&new_ch_row)
        .build();
    sidebar.append(&new_ch_revealer);

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

    let img_sort_btn = gtk4::Button::builder()
        .icon_name("view-sort-descending-symbolic")
        .tooltip_text("Sort images")
        .css_classes(["flat", "koma-ghost-btn"])
        .build();

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
    topbar.append(&img_sort_btn);
    topbar.append(&vsep1);
    topbar.append(&size_scale);
    topbar.append(&vsep2);
    topbar.append(&upload_btn);
    topbar.append(&select_toggle);
    topbar.append(&inspector_toggle);

    let topbar_handle = gtk4::WindowHandle::new();
    topbar_handle.set_valign(gtk4::Align::Start);
    topbar_handle.set_hexpand(true);
    topbar_handle.set_child(Some(&topbar));

    // === MULTI-SELECT FLOATING BAR (SMOOTH SLIDE-UP REVEALER) ===
    let batch_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    batch_bar.add_css_class("koma-floating-dock");

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

    let batch_revealer = gtk4::Revealer::builder()
        .transition_type(gtk4::RevealerTransitionType::SlideUp)
        .transition_duration(160)
        .reveal_child(false)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::End)
        .child(&batch_bar)
        .build();

    // === MAIN LAYOUT ASSEMBLY (floating glass islands + shared right Inspector for Gallery & Lightbox) ===
    let width_sensor = gtk4::DrawingArea::new();
    width_sensor.set_content_height(0);
    width_sensor.set_hexpand(true);

    let canvas_overlay = gtk4::Overlay::new();
    canvas_overlay.set_vexpand(true);
    canvas_overlay.set_hexpand(true);
    canvas_overlay.add_overlay(&topbar_handle);
    canvas_overlay.add_overlay(&batch_revealer);

    let canvas_col = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    canvas_col.add_css_class("koma-gallery-island");
    canvas_col.set_overflow(gtk4::Overflow::Hidden);
    canvas_col.set_hexpand(true);
    canvas_col.set_vexpand(true);
    canvas_col.append(&width_sensor);
    canvas_col.append(&canvas_overlay);

    // Lightbox and Gallery share the right Inspector sidebar while keeping the left channel sidebar open.
    let content_stack = gtk4::Stack::new();
    content_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    content_stack.set_transition_duration(180);
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);
    content_stack.add_named(&canvas_col, Some("gallery"));

    // Pinned right Inspector resizes the gallery cleanly so topbar controls (Select/Zoom) stay visible.
    let inspector_split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk4::PackType::End)
        .pin_sidebar(true)
        .show_sidebar(false)
        .min_sidebar_width(262.0)
        .max_sidebar_width(288.0)
        .hexpand(true)
        .vexpand(true)
        .content(&content_stack)
        .build();

    let left_split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk4::PackType::Start)
        .pin_sidebar(true)
        .show_sidebar(true)
        .min_sidebar_width(212.0)
        .max_sidebar_width(212.0)
        .sidebar_width_fraction(0.0)
        .sidebar(&sidebar)
        .content(&inspector_split)
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
    window.add_css_class("koma-window");
    let saved_theme = store
        .borrow()
        .get_setting("theme")
        .unwrap_or_else(|| "obsidian".to_string());
    apply_theme_class(&window, &saved_theme);

    // Responsive breakpoint for narrow niri splits.
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
        sync,
        sync_status: RefCell::new(SyncStatus::LocalOnly),
        thumb_cache: thumb_cache.clone(),
        channels: RefCell::new(initial_channels),
        channel_sort: Cell::new(saved_channel_sort),
        image_sort: Cell::new(saved_image_sort),
        active_view: Cell::new(initial_view),
        view_mode: RefCell::new(initial_mode.to_string()),
        selected_images: RefCell::new(Vec::new()),
        focused_image: RefCell::new(None),
        select_mode: Cell::new(false),
        suppress_sidebar_select: Cell::new(false),
        suppress_inspector_toggle: Cell::new(false),
        left_split: left_split.clone(),
        sidebar_toggle_btn: sidebar_toggle_btn.clone(),
        sync_pill,
        sync_dot,
        sync_label,
        home_row_btn: home_row_btn.clone(),
        new_ch_revealer: new_ch_revealer.clone(),
        new_ch_entry: new_ch_entry.clone(),
        editing_channel_id: Cell::new(None),
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
        batch_revealer,
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
            move || ctx.set_inspector_visible(false)
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
                    ctx.sync.trigger_sync();
                }
                Err(e) => ctx.toast(&format!("Rename failed: {e}")),
            }
        },
        {
            let ctx = ctx.clone();
            move |image_id, target_cid| ctx.move_images_to(vec![image_id], target_cid)
        },
        {
            let ctx = ctx.clone();
            move |rec| ctx.confirm_delete_ids(vec![rec.id])
        },
    );
    inspector_split.set_sidebar(Some(&inspector_pane.root_widget));
    *ctx.inspector.borrow_mut() = Some(inspector_pane);

    // Build Lightbox inside content_stack so both left sidebar and right Inspector work in Lightbox.
    let lightbox_view = LightboxView::new(
        root_dir,
        thumb_cache,
        {
            let ctx = ctx.clone();
            move || {
                ctx.close_lightbox();
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
        {
            let ctx = ctx.clone();
            move |rec| ctx.confirm_delete_ids(vec![rec.id])
        },
        {
            let ctx = ctx.clone();
            move |show| {
                if show {
                    let ins = ctx.inspector.borrow().clone();
                    if let Some(ins) = ins {
                        let rec = ctx.focused_image.borrow().clone();
                        let chs = ctx.channels.borrow().clone();
                        ins.set_image(rec, &chs);
                    }
                }
                ctx.set_inspector_visible(show);
            }
        },
        {
            let ctx = ctx.clone();
            move |rec| {
                *ctx.focused_image.borrow_mut() = rec.clone();
                if let Some(ins) = ctx.inspector.borrow().as_ref() {
                    ins.set_image(rec, &ctx.channels.borrow());
                }
            }
        },
    );
    content_stack.add_named(&lightbox_view.root_widget, Some("lightbox"));
    *ctx.lightbox.borrow_mut() = Some(lightbox_view);

    // === SIGNAL WIRING ===
    let left_ref = left_split.clone();
    let c_left = ctx.clone();
    sidebar_toggle_btn.connect_toggled(move |btn| {
        let show = btn.is_active();
        if left_ref.shows_sidebar() == show {
            return;
        }
        if !left_ref.is_collapsed()
            && let Some(canvas) = c_left.grid_canvas.borrow().clone()
        {
            canvas.notify_sidebar_animating(if show { -212 } else { 212 });
        }
        left_ref.set_show_sidebar(show);
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
    let ch_sort_anchor = ch_sort_btn.clone();
    ch_sort_btn.connect_clicked(move |_| {
        show_channel_sort_popover(&ch_sort_anchor, &c);
    });

    let c = ctx.clone();
    new_ch_btn.connect_clicked(move |_| c.begin_inline_create_channel());

    let c_nc = ctx.clone();
    new_ch_entry.connect_activate(move |entry| {
        let raw = entry.text().to_string();
        c_nc.new_ch_revealer.set_reveal_child(false);
        if raw.trim().is_empty() {
            return;
        }
        match c_nc.store.borrow().create_channel(&raw) {
            Ok(ch) => {
                c_nc.reload_all();
                c_nc.select_view(ActiveView::Channel(ch.id));
                c_nc.toast(&format!("Created #{}", ch.slug));
                c_nc.sync.trigger_sync();
            }
            Err(e) => c_nc.toast(&format!("Create failed: {e}")),
        }
    });
    let nc_key = gtk4::EventControllerKey::new();
    let rev_esc = new_ch_revealer.clone();
    nc_key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            rev_esc.set_reveal_child(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    new_ch_entry.add_controller(nc_key);
    let nc_focus = gtk4::EventControllerFocus::new();
    let rev_blur = new_ch_revealer;
    nc_focus.connect_leave(move |_| {
        rev_blur.set_reveal_child(false);
    });
    new_ch_entry.add_controller(nc_focus);

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
    let settings_anchor = settings_btn.clone();
    settings_btn.connect_clicked(move |_| {
        show_settings_popover(&settings_anchor, &c);
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
    let img_sort_anchor = img_sort_btn.clone();
    img_sort_btn.connect_clicked(move |_| {
        show_image_sort_popover(&img_sort_anchor, &c);
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
        if c_ins.suppress_inspector_toggle.get() {
            return;
        }
        let show = btn.is_active();
        if show {
            let ins = c_ins.inspector.borrow().clone();
            if let Some(ins) = ins {
                let rec = c_ins.focused_image.borrow().clone();
                let chs = c_ins.channels.borrow().clone();
                ins.set_image(rec, &chs);
            }
        }
        c_ins.set_inspector_visible(show);
    });
    let ins_btn = inspector_toggle.clone();
    let c_ins_sync = ctx.clone();
    inspector_split.connect_show_sidebar_notify(move |split| {
        let showing = split.shows_sidebar();
        c_ins_sync.suppress_inspector_toggle.set(true);
        if ins_btn.is_active() != showing {
            ins_btn.set_active(showing);
        }
        c_ins_sync.suppress_inspector_toggle.set(false);
        if let Some(lb) = c_ins_sync.lightbox.borrow().as_ref() {
            lb.set_inspector_active(showing);
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

    // Global keyboard shortcuts (Ctrl+K, Ctrl+V, Ctrl+A, Ctrl+/-/0, Space, Esc, Lightbox arrows)
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let c = ctx.clone();
    let zoom_scale = size_scale;
    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        let focused_widget = gtk4::prelude::GtkWindowExt::focus(&c.window);
        let editing_text = focused_widget
            .as_ref()
            .is_some_and(|w| w.is::<gtk4::Editable>() || w.is::<gtk4::Text>());

        if !editing_text && key == gdk::Key::F2 {
            let in_sidebar_list = focused_widget
                .as_ref()
                .is_some_and(|w| w == &c.channel_list || c.channel_list.is_ancestor(w));
            if in_sidebar_list {
                if let ActiveView::Channel(cid) = c.active_view.get() {
                    c.begin_inline_rename_channel(cid);
                    return glib::Propagation::Stop;
                }
            }
            let focused_rec = c.focused_image.borrow().clone();
            if let Some(rec) = focused_rec {
                let ins = c.inspector.borrow().clone();
                if let Some(ins) = ins {
                    let channels = c.channels.borrow().clone();
                    ins.set_image(Some(rec), &channels);
                    c.set_inspector_visible(true);
                    ins.begin_inline_rename();
                    return glib::Propagation::Stop;
                }
            } else if let ActiveView::Channel(cid) = c.active_view.get() {
                c.begin_inline_rename_channel(cid);
                return glib::Propagation::Stop;
            }
        }

        if c.content_stack.visible_child_name().as_deref() == Some("lightbox") && !editing_text {
            match key {
                gdk::Key::Escape | gdk::Key::space => {
                    c.close_lightbox();
                    if let Some(canvas) = c.grid_canvas.borrow().as_ref() {
                        canvas.focus_canvas();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Left => {
                    let lb = c.lightbox.borrow().clone();
                    if let Some(lb) = lb {
                        lb.step(-1);
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Right => {
                    let lb = c.lightbox.borrow().clone();
                    if let Some(lb) = lb {
                        lb.step(1);
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::z | gdk::Key::Z => {
                    let lb = c.lightbox.borrow().clone();
                    if let Some(lb) = lb {
                        lb.toggle_zoom();
                    }
                    return glib::Propagation::Stop;
                }
                gdk::Key::Delete => {
                    let rec_id = c
                        .lightbox
                        .borrow()
                        .as_ref()
                        .and_then(|lb| lb.current_record())
                        .map(|r| r.id);
                    if let Some(id) = rec_id {
                        c.confirm_delete_ids(vec![id]);
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(key, gdk::Key::k | gdk::Key::K) {
            c.open_switcher(SwitcherMode::Jump, None);
            return glib::Propagation::Stop;
        }
        if !editing_text && ctrl {
            match key {
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                    let next = (zoom_scale.value().round() + 1.0).clamp(1.0, 7.0);
                    zoom_scale.set_value(next);
                    return glib::Propagation::Stop;
                }
                gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract => {
                    let next = (zoom_scale.value().round() - 1.0).clamp(1.0, 7.0);
                    zoom_scale.set_value(next);
                    return glib::Propagation::Stop;
                }
                gdk::Key::_0 | gdk::Key::KP_0 => {
                    zoom_scale.set_value(4.0);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Up => {
                    c.step_view(-1);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Down => {
                    c.step_view(1);
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }
        if !editing_text && key == gdk::Key::Delete {
            c.confirm_delete_selected();
            return glib::Propagation::Stop;
        }
        if !editing_text && ctrl && matches!(key, gdk::Key::v | gdk::Key::V) {
            c.paste_from_clipboard();
            return glib::Propagation::Stop;
        }
        if !editing_text && ctrl && matches!(key, gdk::Key::a | gdk::Key::A) {
            let canvas = c.grid_canvas.borrow().clone();
            if let Some(canvas) = canvas {
                canvas.select_all();
            }
            return glib::Propagation::Stop;
        }
        if !editing_text
            && key == gdk::Key::space
            && c.content_stack.visible_child_name().as_deref() == Some("gallery")
        {
            let rec = c.focused_image.borrow().clone();
            if let Some(rec) = rec {
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
        let in_sidebar_list = focused_widget
            .as_ref()
            .is_some_and(|w| w == &c.channel_list || c.channel_list.is_ancestor(w));
        if !editing_text
            && !in_sidebar_list
            && c.content_stack.visible_child_name().as_deref() == Some("gallery")
        {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
                let rec = c.focused_image.borrow().clone();
                if let Some(rec) = rec {
                    c.toggle_inspector_for(rec);
                    return glib::Propagation::Stop;
                }
            }
            let dir = match key {
                gdk::Key::Left => Some(NavDir::Left),
                gdk::Key::Right => Some(NavDir::Right),
                gdk::Key::Up => Some(NavDir::Up),
                gdk::Key::Down => Some(NavDir::Down),
                _ => None,
            };
            if let Some(dir) = dir {
                let extend = mods.contains(gdk::ModifierType::SHIFT_MASK);
                let canvas = c.grid_canvas.borrow().clone();
                if let Some(canvas) = canvas
                    && canvas.move_selection(dir, extend)
                {
                    return glib::Propagation::Stop;
                }
            }
        }
        if key == gdk::Key::Escape {
            let sel_len = c.selected_images.borrow().len();
            if c.select_mode.get() || sel_len > 1 {
                c.select_mode.set(false);
                c.select_toggle.set_active(false);
                let canvas = c.grid_canvas.borrow().clone();
                if let Some(canvas) = canvas {
                    canvas.clear_selection();
                }
                c.update_batch_bar();
                return glib::Propagation::Stop;
            } else if c.inspector_split.shows_sidebar() {
                c.set_inspector_visible(false);
                return glib::Propagation::Stop;
            } else if sel_len > 0 {
                let canvas = c.grid_canvas.borrow().clone();
                if let Some(canvas) = canvas {
                    canvas.clear_selection();
                }
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

    ctx.apply_sync_status(SyncStatus::LocalOnly);
    let weak_ctx = Rc::downgrade(&ctx);
    glib::timeout_add_local(Duration::from_millis(250), move || {
        let Some(ctx) = weak_ctx.upgrade() else {
            return glib::ControlFlow::Break;
        };
        for evt in ctx.sync.poll_events() {
            match evt {
                SyncEvent::StatusChanged(status) => ctx.apply_sync_status(status),
                SyncEvent::SyncFinished(report) => {
                    if report.had_local_changes() {
                        ctx.reload_all();
                        if report.downloaded > 0 {
                            ctx.toast(&format!(
                                "Pulled {} image(s) from Google Drive",
                                report.downloaded
                            ));
                        }
                    }
                }
                SyncEvent::AuthFinished(Ok(())) => ctx.toast("Connected to Google Drive"),
                SyncEvent::AuthFinished(Err(e)) => {
                    ctx.toast(&format!("Google Drive auth failed: {e}"))
                }
            }
        }
        glib::ControlFlow::Continue
    });

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

const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("obsidian", "Obsidian"),
    ("inir", "iNiR Glass"),
    ("monochrome", "Monochrome"),
    ("dusk", "Warm Dusk"),
    ("paper", "Manga Paper"),
];

fn apply_theme_class(window: &adw::ApplicationWindow, theme_id: &str) {
    for &(id, _) in BUILTIN_THEMES {
        if id != "inir" {
            window.remove_css_class(&format!("theme-{id}"));
        }
    }
    if theme_id != "inir" && BUILTIN_THEMES.iter().any(|&(id, _)| id == theme_id) {
        window.add_css_class(&format!("theme-{theme_id}"));
    }
    let scheme = match theme_id {
        "paper" => adw::ColorScheme::ForceLight,
        "inir" => adw::ColorScheme::Default,
        _ => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn show_settings_popover(anchor: &gtk4::Button, ctx: &Rc<AppCtx>) {
    let popover = gtk4::Popover::new();
    popover.add_css_class("koma-settings-pop");
    popover.set_parent(anchor);
    popover.set_position(gtk4::PositionType::Top);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
    root.set_size_request(248, -1);

    let theme_heading = gtk4::Label::builder()
        .label("THEME")
        .xalign(0.0)
        .css_classes(["koma-section-label"])
        .build();
    root.append(&theme_heading);

    let current_theme = ctx
        .store
        .borrow()
        .get_setting("theme")
        .unwrap_or_else(|| "obsidian".to_string());

    let theme_grid = gtk4::Grid::builder()
        .column_spacing(6)
        .row_spacing(6)
        .column_homogeneous(true)
        .build();

    let mut group_head: Option<gtk4::ToggleButton> = None;
    for (idx, &(id, label)) in BUILTIN_THEMES.iter().enumerate() {
        let btn = gtk4::ToggleButton::builder()
            .label(label)
            .active(current_theme == id)
            .css_classes(["flat", "koma-theme-btn"])
            .build();
        if let Some(ref head) = group_head {
            btn.set_group(Some(head));
        } else {
            group_head = Some(btn.clone());
        }
        let ctx_t = ctx.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                apply_theme_class(&ctx_t.window, id);
                let _ = ctx_t.store.borrow().set_setting("theme", id);
            }
        });
        let span = if idx + 1 == BUILTIN_THEMES.len() && BUILTIN_THEMES.len() % 2 == 1 {
            2
        } else {
            1
        };
        theme_grid.attach(&btn, (idx % 2) as i32, (idx / 2) as i32, span, 1);
    }
    root.append(&theme_grid);

    let sep1 = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    sep1.add_css_class("koma-switcher-sep");
    root.append(&sep1);

    let drive_heading = gtk4::Label::builder()
        .label("GOOGLE DRIVE")
        .xalign(0.0)
        .css_classes(["koma-section-label"])
        .build();
    root.append(&drive_heading);

    let status_text = match &*ctx.sync_status.borrow() {
        SyncStatus::LocalOnly => "Not connected (local only)".to_string(),
        SyncStatus::Syncing => "Syncing with Google Drive...".to_string(),
        SyncStatus::Synced(summary) => summary.clone(),
        SyncStatus::Offline(err) => format!("Offline: {err}"),
    };
    let drive_status_label = gtk4::Label::builder()
        .label(&status_text)
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(32)
        .css_classes(["koma-meta-val"])
        .build();
    root.append(&drive_status_label);

    let is_configured = GDriveClientConfig::is_configured();
    let is_connected = gdrive::has_saved_tokens(ctx.store.borrow().root());

    let import_json_dialog = |ctx_i: Rc<AppCtx>, pop_i: gtk4::Popover| {
        pop_i.popdown();
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some("Google OAuth JSON"));
        filter.add_suffix("json");
        let filters = gio::ListStore::new::<gtk4::FileFilter>();
        filters.append(&filter);
        let dialog = gtk4::FileDialog::builder()
            .title("Select Google OAuth Desktop Client JSON")
            .filters(&filters)
            .build();
        let win = ctx_i.window.clone();
        dialog.open(Some(&win), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            match gdrive::import_client_json(&path) {
                Ok(_) => ctx_i.toast("Saved Google Drive client config; open settings to connect"),
                Err(e) => ctx_i.toast(&format!("Import failed: {e}")),
            }
        });
    };

    if !is_configured {
        let import_btn = gtk4::Button::builder()
            .label("Import Client JSON")
            .css_classes(["flat", "koma-action-btn"])
            .build();
        let ctx_i = ctx.clone();
        let pop_i = popover.clone();
        import_btn.connect_clicked(move |_| import_json_dialog(ctx_i.clone(), pop_i.clone()));
        root.append(&import_btn);

        let hint = gtk4::Label::builder()
            .label("Desktop OAuth JSON -> ~/.config/koma/gdrive_client.json")
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(34)
            .css_classes(["koma-tabular"])
            .build();
        root.append(&hint);
    } else if !is_connected {
        let connect_btn = gtk4::Button::builder()
            .label("Connect Google Drive")
            .css_classes(["flat", "koma-accent-btn"])
            .build();
        let ctx_c = ctx.clone();
        let pop_c = popover.clone();
        connect_btn.connect_clicked(move |_| {
            pop_c.popdown();
            ctx_c.toast("Opening browser to connect Google Drive...");
            ctx_c.sync.start_oauth();
        });
        root.append(&connect_btn);

        let replace_btn = gtk4::Button::builder()
            .label("Replace Client JSON")
            .css_classes(["flat", "koma-action-subtle"])
            .build();
        let ctx_i = ctx.clone();
        let pop_i = popover.clone();
        replace_btn.connect_clicked(move |_| import_json_dialog(ctx_i.clone(), pop_i.clone()));
        root.append(&replace_btn);
    } else {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let sync_now_btn = gtk4::Button::builder()
            .label("Sync Now")
            .hexpand(true)
            .css_classes(["flat", "koma-action-btn"])
            .build();
        let disconnect_btn = gtk4::Button::builder()
            .label("Disconnect")
            .css_classes(["flat", "koma-action-subtle", "destructive-action"])
            .build();
        row.append(&sync_now_btn);
        row.append(&disconnect_btn);
        root.append(&row);

        let ctx_s = ctx.clone();
        let pop_s = popover.clone();
        sync_now_btn.connect_clicked(move |_| {
            pop_s.popdown();
            ctx_s.sync.trigger_sync();
        });

        let ctx_d = ctx.clone();
        let pop_d = popover.clone();
        disconnect_btn.connect_clicked(move |_| {
            pop_d.popdown();
            ctx_d.sync.disconnect();
            ctx_d.toast("Disconnected Google Drive");
        });
    }

    let sep_drive = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    sep_drive.add_css_class("koma-switcher-sep");
    root.append(&sep_drive);

    let storage_heading = gtk4::Label::builder()
        .label("STORAGE")
        .xalign(0.0)
        .css_classes(["koma-section-label"])
        .build();
    let dir_label = gtk4::Label::builder()
        .label(ctx.store.borrow().root().display().to_string())
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .css_classes(["koma-meta-val"])
        .build();
    root.append(&storage_heading);
    root.append(&dir_label);

    let sep2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    sep2.add_css_class("koma-switcher-sep");
    root.append(&sep2);

    let keys_heading = gtk4::Label::builder()
        .label("SHORTCUTS")
        .xalign(0.0)
        .css_classes(["koma-section-label"])
        .build();
    let keys_body = gtk4::Label::builder()
        .label("Ctrl+V paste · Ctrl+K jump · Ctrl+↑/↓ channels\nCtrl+A select all · Ctrl +/- zoom · F2 rename\nEnter inspect · Space lightbox · Del delete")
        .xalign(0.0)
        .css_classes(["koma-tabular"])
        .build();
    root.append(&keys_heading);
    root.append(&keys_body);

    popover.set_child(Some(&root));
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}

fn show_channel_sort_popover(anchor: &gtk4::Button, ctx: &Rc<AppCtx>) {
    let popover = gtk4::Popover::new();
    popover.add_css_class("koma-settings-pop");
    popover.add_css_class("koma-menu-pop");
    popover.set_has_arrow(false);
    popover.set_parent(anchor);
    popover.set_position(gtk4::PositionType::Bottom);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    root.set_size_request(176, -1);
    let heading = gtk4::Label::builder()
        .label("SORT CHANNELS")
        .xalign(0.0)
        .margin_start(8)
        .margin_top(4)
        .margin_bottom(4)
        .css_classes(["koma-section-label"])
        .build();
    root.append(&heading);

    let current = ctx.channel_sort.get();
    let options = [
        (ChannelSort::Count, "Most images"),
        (ChannelSort::Name, "Name (A to Z)"),
        (ChannelSort::Recent, "Recently updated"),
        (ChannelSort::Custom, "Custom (drag)"),
    ];
    for (opt, label) in options {
        let lbl = gtk4::Label::builder()
            .label(if opt == current {
                format!("✓  {label}")
            } else {
                format!("    {label}")
            })
            .xalign(0.0)
            .build();
        let btn = gtk4::Button::builder()
            .child(&lbl)
            .css_classes(["flat", "koma-menu-item"])
            .build();
        let ctx_s = ctx.clone();
        let pop_s = popover.clone();
        btn.connect_clicked(move |_| {
            pop_s.popdown();
            ctx_s.channel_sort.set(opt);
            let _ = ctx_s
                .store
                .borrow()
                .set_setting("channel_sort", opt.as_str());
            ctx_s.reload_all();
        });
        root.append(&btn);
    }

    popover.set_child(Some(&root));
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}

fn show_image_sort_popover(anchor: &gtk4::Button, ctx: &Rc<AppCtx>) {
    let popover = gtk4::Popover::new();
    popover.add_css_class("koma-settings-pop");
    popover.add_css_class("koma-menu-pop");
    popover.set_has_arrow(false);
    popover.set_parent(anchor);
    popover.set_position(gtk4::PositionType::Bottom);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    root.set_size_request(172, -1);
    let heading = gtk4::Label::builder()
        .label("SORT IMAGES")
        .xalign(0.0)
        .margin_start(8)
        .margin_top(4)
        .margin_bottom(4)
        .css_classes(["koma-section-label"])
        .build();
    root.append(&heading);

    let current = ctx.image_sort.get();
    let options = [
        (ImageSort::Newest, "Newest first"),
        (ImageSort::Oldest, "Oldest first"),
    ];
    for (opt, label) in options {
        let lbl = gtk4::Label::builder()
            .label(if opt == current {
                format!("✓  {label}")
            } else {
                format!("    {label}")
            })
            .xalign(0.0)
            .build();
        let btn = gtk4::Button::builder()
            .child(&lbl)
            .css_classes(["flat", "koma-menu-item"])
            .build();
        let ctx_s = ctx.clone();
        let pop_s = popover.clone();
        btn.connect_clicked(move |_| {
            pop_s.popdown();
            ctx_s.image_sort.set(opt);
            let _ = ctx_s.store.borrow().set_setting("image_sort", opt.as_str());
            ctx_s.reload_images_with_scroll(false);
        });
        root.append(&btn);
    }

    popover.set_child(Some(&root));
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}

fn present_confirm_dialog(
    parent: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::Dialog::builder()
        .content_width(340)
        .follows_content_size(true)
        .build();

    let box_root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    box_root.add_css_class("koma-dialog");

    let title = gtk4::Label::builder()
        .label(heading)
        .xalign(0.0)
        .css_classes(["koma-channel-title"])
        .build();
    let desc = gtk4::Label::builder()
        .label(body)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["koma-dialog-body"])
        .build();

    let btn_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk4::Align::End);
    btn_row.set_margin_top(4);

    let cancel_btn = gtk4::Button::builder()
        .label("Cancel")
        .css_classes(["flat", "koma-action-subtle"])
        .build();
    let confirm_btn = gtk4::Button::builder()
        .label(confirm_label)
        .css_classes(["flat", "koma-danger-btn"])
        .build();

    btn_row.append(&cancel_btn);
    btn_row.append(&confirm_btn);

    box_root.append(&title);
    box_root.append(&desc);
    box_root.append(&btn_row);
    dialog.set_child(Some(&box_root));

    let d_cancel = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        d_cancel.close();
    });

    let confirmed = Rc::new(Cell::new(false));
    let fire_confirm: Rc<dyn Fn()> = Rc::new(move || {
        if !confirmed.replace(true) {
            on_confirm();
        }
    });

    let d_ok = dialog.clone();
    let ok_cb = fire_confirm.clone();
    confirm_btn.connect_clicked(move |_| {
        d_ok.close();
        ok_cb();
    });

    let key_ctrl = gtk4::EventControllerKey::new();
    let d_key = dialog.clone();
    key_ctrl.connect_key_pressed(move |_, key, _, _| match key {
        gdk::Key::Escape => {
            d_key.close();
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter => {
            d_key.close();
            fire_confirm();
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    box_root.add_controller(key_ctrl);

    dialog.present(Some(parent));
    let focus_btn = confirm_btn.clone();
    glib::idle_add_local_once(move || {
        focus_btn.grab_focus();
    });
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
