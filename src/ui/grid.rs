use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

use crate::store::ImageRecord;
use crate::ui::media_util::AsyncThumbLoader;

const ROW_GAP: i32 = 6;
/// Horizontal margin (14px * 2) + vertical scrollbar clearance (14px).
const CANVAS_PAD_X: i32 = 42;
const INITIAL_TIMELINE_ROWS: usize = 36;

#[derive(Clone, Debug)]
struct RowTile {
    index: usize,
    record: ImageRecord,
    width: i32,
}

#[derive(Clone, Debug)]
struct JustifiedRow {
    height: i32,
    tiles: Vec<RowTile>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RowObject {
        pub(super) row: RefCell<Option<JustifiedRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowObject {
        const NAME: &'static str = "KomaJustifiedRow";
        type Type = super::RowObject;
    }

    impl ObjectImpl for RowObject {}
}

glib::wrapper! {
    pub struct RowObject(ObjectSubclass<imp::RowObject>);
}

impl RowObject {
    fn new(row: JustifiedRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row));
        obj
    }

    fn row(&self) -> JustifiedRow {
        self.imp()
            .row
            .borrow()
            .clone()
            .expect("RowObject initialized with row")
    }
}

pub struct GridCanvas {
    pub stack: gtk4::Stack,
    row_store: gio::ListStore,
    list_view: gtk4::ListView,
    grid_scroll: gtk4::ScrolledWindow,
    timeline_box: gtk4::Box,
    timeline_scroll: gtk4::ScrolledWindow,
    records: Rc<RefCell<Vec<ImageRecord>>>,
    selected_ids: Rc<RefCell<HashSet<i64>>>,
    last_clicked_index: Rc<Cell<Option<usize>>>,
    /// Zoom level 1..=7 (1 = zoomed out compact rows, 7 = zoomed in large rows).
    zoom_level: Rc<Cell<i32>>,
    viewport_width: Rc<Cell<i32>>,
    select_mode: Rc<Cell<bool>>,
    show_channel_badges: Rc<Cell<bool>>,
    timeline_row_limit: Rc<Cell<usize>>,
    root: PathBuf,
    loader: Rc<AsyncThumbLoader>,
    on_selection_change: Rc<RefCell<Box<dyn Fn(Vec<ImageRecord>, Option<ImageRecord>)>>>,
    on_activate_image: Rc<RefCell<Box<dyn Fn(ImageRecord)>>>,
    on_jump_channel: Rc<RefCell<Box<dyn Fn(i64)>>>,
}

impl GridCanvas {
    pub fn new(
        root: PathBuf,
        loader: Rc<AsyncThumbLoader>,
        on_selection_change: impl Fn(Vec<ImageRecord>, Option<ImageRecord>) + 'static,
        on_activate_image: impl Fn(ImageRecord) + 'static,
        on_jump_channel: impl Fn(i64) + 'static,
    ) -> Rc<Self> {
        let row_store = gio::ListStore::new::<RowObject>();
        let no_sel = gtk4::NoSelection::new(Some(row_store.clone()));
        let zoom_level = Rc::new(Cell::new(4));
        let viewport_width = Rc::new(Cell::new(920));
        let select_mode = Rc::new(Cell::new(false));
        let show_channel_badges = Rc::new(Cell::new(false));
        let records: Rc<RefCell<Vec<ImageRecord>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_ids: Rc<RefCell<HashSet<i64>>> = Rc::new(RefCell::new(HashSet::new()));
        let last_clicked_index = Rc::new(Cell::new(None));
        let timeline_row_limit = Rc::new(Cell::new(INITIAL_TIMELINE_ROWS));

        let on_selection_change: Rc<RefCell<Box<dyn Fn(Vec<ImageRecord>, Option<ImageRecord>)>>> =
            Rc::new(RefCell::new(Box::new(on_selection_change)));
        let on_activate_image: Rc<RefCell<Box<dyn Fn(ImageRecord)>>> =
            Rc::new(RefCell::new(Box::new(on_activate_image)));
        let on_jump_channel: Rc<RefCell<Box<dyn Fn(i64)>>> =
            Rc::new(RefCell::new(Box::new(on_jump_channel)));

        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(|_, obj| {
            let item = obj
                .downcast_ref::<gtk4::ListItem>()
                .expect("ListItem in setup");
            item.set_activatable(false);
            item.set_selectable(false);
            item.set_focusable(false);
            let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, ROW_GAP);
            row_box.set_halign(gtk4::Align::Start);
            item.set_child(Some(&row_box));
        });

        let list_view = gtk4::ListView::builder()
            .model(&no_sel)
            .factory(&factory)
            .single_click_activate(false)
            .focusable(true)
            .can_focus(true)
            .css_classes(["koma-gallery-list"])
            .build();

        // PolicyType::External hides horizontal scrollbar while allowing the viewport to shrink
        // freely when Inspector opens or niri splits narrow.
        let grid_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::External)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .propagate_natural_width(false)
            .min_content_width(220)
            .vexpand(true)
            .hexpand(true)
            .child(&list_view)
            .build();

        let timeline_box = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
        timeline_box.set_margin_top(58);
        timeline_box.set_margin_bottom(36);
        timeline_box.set_margin_start(14);
        timeline_box.set_margin_end(14);

        let timeline_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::External)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .propagate_natural_width(false)
            .min_content_width(220)
            .focusable(true)
            .can_focus(true)
            .vexpand(true)
            .hexpand(true)
            .child(&timeline_box)
            .build();

        let empty_page = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        empty_page.set_valign(gtk4::Align::Center);
        empty_page.set_halign(gtk4::Align::Center);
        let empty_title = gtk4::Label::builder()
            .label("No images in this channel yet")
            .css_classes(["title-3"])
            .build();
        let empty_sub = gtk4::Label::builder()
            .label("Press Ctrl+V to paste a screenshot or drop files anywhere.")
            .css_classes(["dim-label"])
            .build();
        empty_page.append(&empty_title);
        empty_page.append(&empty_sub);

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_named(&grid_scroll, Some("grid"));
        stack.add_named(&timeline_scroll, Some("timeline"));
        stack.add_named(&empty_page, Some("empty"));

        let canvas = Rc::new(Self {
            stack,
            row_store,
            list_view,
            grid_scroll,
            timeline_box,
            timeline_scroll,
            records,
            selected_ids,
            last_clicked_index,
            zoom_level,
            viewport_width,
            select_mode,
            show_channel_badges,
            timeline_row_limit,
            root,
            loader,
            on_selection_change,
            on_activate_image,
            on_jump_channel,
        });

        let c_bind = canvas.clone();
        factory.connect_bind(move |_, obj| {
            let item = obj
                .downcast_ref::<gtk4::ListItem>()
                .expect("ListItem in bind");
            let Some(row_obj) = item.item().and_downcast::<RowObject>() else {
                return;
            };
            let Some(row_box) = item.child().and_downcast::<gtk4::Box>() else {
                return;
            };
            c_bind.populate_justified_row(&row_box, &row_obj.row(), c_bind.show_channel_badges.get());
        });

        factory.connect_unbind(|_, obj| {
            let item = obj
                .downcast_ref::<gtk4::ListItem>()
                .expect("ListItem in unbind");
            if let Some(row_box) = item.child().and_downcast::<gtk4::Box>() {
                while let Some(child) = row_box.first_child() {
                    if let Some(card) = child.downcast_ref::<gtk4::Box>()
                        && let Some(overlay) = card.first_child().and_downcast::<gtk4::Overlay>()
                        && let Some(picture) = overlay.child().and_downcast::<gtk4::Picture>()
                    {
                        picture.set_widget_name("");
                        picture.set_paintable(gdk::Paintable::NONE);
                    }
                    row_box.remove(&child);
                }
            }
        });

        canvas
    }

    fn populate_justified_row(
        self: &Rc<Self>,
        row_box: &gtk4::Box,
        row: &JustifiedRow,
        with_channel_badge: bool,
    ) {
        while let Some(child) = row_box.first_child() {
            row_box.remove(&child);
        }

        let selected_snapshot = self.selected_ids.borrow().clone();
        let show_checks = self.select_mode.get() || selected_snapshot.len() > 1;

        for tile in &row.tiles {
            let is_sel = selected_snapshot.contains(&tile.record.id);
            let card = self.build_tile_card(
                tile.index,
                &tile.record,
                tile.width,
                row.height,
                is_sel,
                show_checks,
                with_channel_badge,
            );
            row_box.append(&card);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tile_card(
        self: &Rc<Self>,
        global_idx: usize,
        rec: &ImageRecord,
        tile_w: i32,
        tile_h: i32,
        is_sel: bool,
        show_check: bool,
        with_channel_badge: bool,
    ) -> gtk4::Box {
        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        card.set_halign(gtk4::Align::Start);
        card.set_widget_name(&rec.id.to_string());
        if is_sel {
            card.add_css_class("koma-tile-selected");
        }

        let picture = gtk4::Picture::new();
        picture.set_content_fit(gtk4::ContentFit::Cover);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        let thumb_abs = self.root.join(&rec.thumb_path);
        self.loader.bind_picture(&picture, &thumb_abs);

        let check = gtk4::Label::builder()
            .label("✓")
            .name("koma-check")
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .visible(is_sel && show_check)
            .css_classes(["koma-check-badge"])
            .build();

        // Hover-only bottom scrim keeps the gallery borderless and free of permanent text static.
        let scrim = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        scrim.add_css_class("koma-tile-scrim");
        scrim.set_valign(gtk4::Align::End);
        scrim.set_hexpand(true);
        scrim.set_can_target(false);
        let caption = gtk4::Label::builder()
            .label(&rec.filename)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .single_line_mode(true)
            .css_classes(["koma-tile-caption"])
            .build();
        scrim.append(&caption);

        let overlay = gtk4::Overlay::new();
        overlay.set_size_request(tile_w, tile_h);
        overlay.set_overflow(gtk4::Overflow::Hidden);
        overlay.add_css_class("koma-tile-frame");
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&scrim);
        overlay.add_overlay(&check);

        if with_channel_badge {
            let ch_btn = gtk4::Button::builder()
                .label(format!("#{}", rec.channel_slug))
                .halign(gtk4::Align::Start)
                .valign(gtk4::Align::Start)
                .css_classes(["flat", "koma-tile-ch-tag"])
                .build();
            let cid = rec.channel_id;
            let jump_cb = self.on_jump_channel.clone();
            ch_btn.connect_clicked(move |_| {
                (jump_cb.borrow())(cid);
            });
            overlay.add_overlay(&ch_btn);
        }

        card.append(&overlay);

        let click = gtk4::GestureClick::new();
        let c_click = self.clone();
        let rec_click = rec.clone();
        click.connect_released(move |gesture, n_press, _, _| {
            if n_press >= 2 {
                c_click.select_single(global_idx, &rec_click);
                (c_click.on_activate_image.borrow())(rec_click.clone());
                return;
            }
            let mods = gesture.current_event_state();
            c_click.handle_tile_click(global_idx, &rec_click, mods);
        });
        overlay.add_controller(click);
        card
    }

    fn select_single(self: &Rc<Self>, idx: usize, rec: &ImageRecord) {
        {
            let mut ids = self.selected_ids.borrow_mut();
            ids.clear();
            ids.insert(rec.id);
        }
        self.last_clicked_index.set(Some(idx));
        self.refresh_selection_visuals();
        (self.on_selection_change.borrow())(vec![rec.clone()], Some(rec.clone()));
    }

    fn handle_tile_click(
        self: &Rc<Self>,
        idx: usize,
        rec: &ImageRecord,
        mods: gdk::ModifierType,
    ) {
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let multi = self.select_mode.get() || ctrl;

        {
            let mut ids = self.selected_ids.borrow_mut();
            if shift
                && let Some(anchor) = self.last_clicked_index.get()
            {
                let (lo, hi) = if anchor <= idx {
                    (anchor, idx)
                } else {
                    (idx, anchor)
                };
                if !ctrl && !self.select_mode.get() {
                    ids.clear();
                }
                let recs = self.records.borrow();
                for r in recs.iter().skip(lo).take(hi - lo + 1) {
                    ids.insert(r.id);
                }
            } else if multi {
                if !ids.remove(&rec.id) {
                    ids.insert(rec.id);
                }
                self.last_clicked_index.set(Some(idx));
            } else {
                ids.clear();
                ids.insert(rec.id);
                self.last_clicked_index.set(Some(idx));
            }
        }

        let selected_list: Vec<ImageRecord> = {
            let ids = self.selected_ids.borrow();
            self.records
                .borrow()
                .iter()
                .filter(|r| ids.contains(&r.id))
                .cloned()
                .collect()
        };
        self.refresh_selection_visuals();
        (self.on_selection_change.borrow())(selected_list, Some(rec.clone()));
    }

    fn update_row_selection(row_box: &gtk4::Box, ids: &HashSet<i64>, show_checks: bool) {
        let mut tile = row_box.first_child();
        while let Some(tw) = tile {
            if let Ok(id) = tw.widget_name().parse::<i64>() {
                let sel = ids.contains(&id);
                if sel {
                    tw.add_css_class("koma-tile-selected");
                } else {
                    tw.remove_css_class("koma-tile-selected");
                }
                if let Some(overlay) = tw.first_child().and_downcast::<gtk4::Overlay>() {
                    let mut child = overlay.first_child();
                    while let Some(c) = child {
                        if c.widget_name() == "koma-check" {
                            c.set_visible(sel && show_checks);
                            break;
                        }
                        child = c.next_sibling();
                    }
                }
            }
            tile = tw.next_sibling();
        }
    }

    fn refresh_selection_visuals(&self) {
        let ids = self.selected_ids.borrow().clone();
        let show_checks = self.select_mode.get() || ids.len() > 1;

        if self.stack.visible_child_name().as_deref() == Some("grid") {
            let mut row_widget = self.list_view.first_child();
            while let Some(rw) = row_widget {
                if let Some(row_box) = rw.first_child().and_downcast::<gtk4::Box>() {
                    Self::update_row_selection(&row_box, &ids, show_checks);
                }
                row_widget = rw.next_sibling();
            }
        } else if self.stack.visible_child_name().as_deref() == Some("timeline") {
            let mut sec = self.timeline_box.first_child();
            while let Some(sw) = sec {
                if let Some(sec_box) = sw.downcast_ref::<gtk4::Box>() {
                    let mut child = sec_box.first_child();
                    while let Some(cw) = child {
                        if let Some(row_box) = cw.downcast_ref::<gtk4::Box>()
                            && row_box.orientation() == gtk4::Orientation::Horizontal
                        {
                            Self::update_row_selection(row_box, &ids, show_checks);
                        }
                        child = cw.next_sibling();
                    }
                }
                sec = sw.next_sibling();
            }
        }
    }

    pub fn focus_widget(&self) -> gtk4::Widget {
        if self.stack.visible_child_name().as_deref() == Some("grid") {
            self.list_view.clone().upcast()
        } else {
            self.timeline_scroll.clone().upcast()
        }
    }

    pub fn focus_canvas(&self) {
        self.focus_widget().grab_focus();
    }

    pub fn set_viewport_width(self: &Rc<Self>, width: i32) {
        let usable = (width - CANVAS_PAD_X).max(260);
        if (self.viewport_width.get() - usable).abs() < 8 {
            return;
        }
        self.viewport_width.set(usable);
        match self.stack.visible_child_name().as_deref() {
            Some("grid") => self.repack_rows(),
            Some("timeline") => self.rebuild_timeline(),
            _ => {}
        }
    }

    pub fn set_records(
        self: &Rc<Self>,
        recs: Vec<ImageRecord>,
        view_mode: &str,
        is_home: bool,
    ) {
        self.loader.bump_generation();
        self.selected_ids.borrow_mut().clear();
        self.last_clicked_index.set(None);
        self.show_channel_badges.set(is_home);
        self.timeline_row_limit.set(INITIAL_TIMELINE_ROWS);
        *self.records.borrow_mut() = recs;

        if self.records.borrow().is_empty() {
            self.row_store.remove_all();
            self.stack.set_visible_child_name("empty");
            (self.on_selection_change.borrow())(Vec::new(), None);
            return;
        }

        self.set_mode(view_mode);
        let first = self.records.borrow().first().cloned();
        (self.on_selection_change.borrow())(Vec::new(), first);
    }

    pub fn set_mode(self: &Rc<Self>, view_mode: &str) {
        if self.records.borrow().is_empty() {
            self.stack.set_visible_child_name("empty");
            return;
        }
        if view_mode == "timeline" {
            self.rebuild_timeline();
            self.stack.set_visible_child_name("timeline");
        } else {
            self.repack_rows();
            self.stack.set_visible_child_name("grid");
            self.grid_scroll.vadjustment().set_value(0.0);
        }
    }

    pub fn set_zoom_level(self: &Rc<Self>, level: i32) {
        let clamped = level.clamp(1, 7);
        if self.zoom_level.get() == clamped {
            return;
        }
        self.zoom_level.set(clamped);

        match self.stack.visible_child_name().as_deref() {
            Some("grid") => self.repack_rows(),
            Some("timeline") => self.rebuild_timeline(),
            _ => {}
        }
    }

    pub fn set_select_mode(self: &Rc<Self>, enabled: bool) {
        self.select_mode.set(enabled);
        self.refresh_selection_visuals();
    }

    pub fn select_all(self: &Rc<Self>) {
        let recs = self.records.borrow().clone();
        if recs.is_empty() {
            return;
        }
        {
            let mut ids = self.selected_ids.borrow_mut();
            ids.clear();
            for r in &recs {
                ids.insert(r.id);
            }
        }
        let focused = recs.first().cloned();
        self.refresh_selection_visuals();
        (self.on_selection_change.borrow())(recs, focused);
    }

    pub fn clear_selection(self: &Rc<Self>) {
        self.selected_ids.borrow_mut().clear();
        self.last_clicked_index.set(None);
        self.refresh_selection_visuals();
        let focused = self.records.borrow().first().cloned();
        (self.on_selection_change.borrow())(Vec::new(), focused);
    }

    pub fn records_snapshot(&self) -> Vec<ImageRecord> {
        self.records.borrow().clone()
    }

    pub fn index_of(&self, image_id: i64) -> Option<usize> {
        self.records.borrow().iter().position(|r| r.id == image_id)
    }

    fn repack_rows(&self) {
        let recs = self.records.borrow();
        let vw = self.viewport_width.get();
        let target_h = target_row_height(self.zoom_level.get(), vw);
        let rows = pack_justified_rows(&recs, 0, vw, target_h, ROW_GAP);
        let objects: Vec<RowObject> = rows.into_iter().map(RowObject::new).collect();
        let old_len = self.row_store.n_items();
        self.row_store.splice(0, old_len, &objects);
    }

    fn rebuild_timeline(self: &Rc<Self>) {
        while let Some(child) = self.timeline_box.first_child() {
            self.timeline_box.remove(&child);
        }

        let recs = self.records.borrow().clone();
        if recs.is_empty() {
            return;
        }

        // Group into Today / Yesterday / Month buckets so 1-image days pack tightly without
        // wasting full horizontal lines on single images.
        let mut groups: Vec<(String, Vec<(usize, ImageRecord)>)> = Vec::new();
        for (idx, rec) in recs.into_iter().enumerate() {
            let bucket = timeline_bucket_label(rec.created_at);
            if let Some(last) = groups.last_mut()
                && last.0 == bucket
            {
                last.1.push((idx, rec));
            } else {
                groups.push((bucket, vec![(idx, rec)]));
            }
        }

        let vw = self.viewport_width.get();
        let target_h = target_row_height(self.zoom_level.get(), vw);
        let with_badges = self.show_channel_badges.get();
        let row_limit = self.timeline_row_limit.get();
        let mut rows_rendered = 0usize;
        let mut has_more = false;

        for (label, items) in groups {
            if rows_rendered >= row_limit {
                has_more = true;
                break;
            }

            let section = gtk4::Box::new(gtk4::Orientation::Vertical, 6);

            let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
            header.set_margin_bottom(2);
            let title = gtk4::Label::builder()
                .label(label.to_uppercase())
                .css_classes(["koma-timeline-title"])
                .build();
            let count = gtk4::Label::builder()
                .label(items.len().to_string())
                .css_classes(["koma-tabular"])
                .build();
            let rule = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            rule.add_css_class("koma-timeline-rule");
            rule.set_hexpand(true);
            rule.set_valign(gtk4::Align::Center);
            header.append(&title);
            header.append(&count);
            header.append(&rule);
            section.append(&header);

            let packed = pack_indexed_justified_rows(&items, vw, target_h, ROW_GAP);
            for row in packed {
                if rows_rendered >= row_limit {
                    has_more = true;
                    break;
                }
                let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, ROW_GAP);
                row_box.set_halign(gtk4::Align::Start);
                self.populate_justified_row(&row_box, &row, with_badges);
                section.append(&row_box);
                rows_rendered += 1;
            }

            self.timeline_box.append(&section);
        }

        if has_more {
            let more_btn = gtk4::Button::builder()
                .label("Load older images")
                .halign(gtk4::Align::Center)
                .margin_top(8)
                .css_classes(["pill"])
                .build();
            let c_more = self.clone();
            more_btn.connect_clicked(move |_| {
                c_more
                    .timeline_row_limit
                    .set(c_more.timeline_row_limit.get() + INITIAL_TIMELINE_ROWS);
                c_more.rebuild_timeline();
            });
            self.timeline_box.append(&more_btn);
        }
    }
}

fn timeline_bucket_label(unix_secs: i64) -> String {
    let Ok(dt) = glib::DateTime::from_unix_local(unix_secs) else {
        return "Earlier".to_string();
    };
    let Ok(now) = glib::DateTime::now_local() else {
        return dt
            .format("%B %Y")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "Earlier".to_string());
    };
    if dt.ymd() == now.ymd() {
        "Today".to_string()
    } else if let Ok(yesterday) = now.add_days(-1)
        && dt.ymd() == yesterday.ymd()
    {
        "Yesterday".to_string()
    } else {
        dt.format("%B %Y")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "Earlier".to_string())
    }
}

fn clamped_aspect(width: u32, height: u32) -> f64 {
    if height == 0 || width == 0 {
        return 1.0;
    }
    ((width as f64) / (height as f64)).clamp(0.48, 2.5)
}

fn target_row_height(level: i32, viewport_width: i32) -> i32 {
    let base: f64 = match level {
        1 => 108.0,
        2 => 132.0,
        3 => 158.0,
        4 => 186.0,
        5 => 218.0,
        6 => 256.0,
        _ => 300.0,
    };
    // Always auto-scale on wider windows so fullscreen shows 4-5 bold tiles per row
    // instead of packing 7-8 tiny tiles across the width.
    let scale = ((viewport_width as f64) / 760.0).clamp(0.95, 1.44);
    (base * scale).round() as i32
}

fn pack_justified_rows(
    records: &[ImageRecord],
    index_offset: usize,
    container_width: i32,
    target_height: i32,
    gap: i32,
) -> Vec<JustifiedRow> {
    let indexed: Vec<(usize, ImageRecord)> = records
        .iter()
        .enumerate()
        .map(|(i, r)| (i + index_offset, r.clone()))
        .collect();
    pack_indexed_justified_rows(&indexed, container_width, target_height, gap)
}

fn pack_indexed_justified_rows(
    items: &[(usize, ImageRecord)],
    container_width: i32,
    target_height: i32,
    gap: i32,
) -> Vec<JustifiedRow> {
    if items.is_empty() || container_width <= 0 {
        return Vec::new();
    }

    let cw = container_width as f64;
    let th = target_height as f64;
    let g = gap as f64;

    let mut rows = Vec::new();
    let mut current: Vec<(usize, &ImageRecord, f64)> = Vec::new();
    let mut aspect_sum = 0.0f64;

    for (idx, rec) in items {
        let aspect = clamped_aspect(rec.width, rec.height);
        current.push((*idx, rec, aspect));
        aspect_sum += aspect;

        let gaps_w = g * ((current.len().saturating_sub(1)) as f64);
        let natural_w = aspect_sum * th + gaps_w;

        if natural_w >= cw {
            let exact_h = ((cw - gaps_w) / aspect_sum).clamp(th * 0.6, th * 1.35);
            rows.push(finalize_row(&current, exact_h, Some(container_width), gap));
            current.clear();
            aspect_sum = 0.0;
        }
    }

    if !current.is_empty() {
        // Clamp final partial row so even a single ultra-wide image never overflows container_width.
        let gaps_w = g * ((current.len().saturating_sub(1)) as f64);
        let natural_w = aspect_sum * th + gaps_w;
        let final_h = if natural_w > cw {
            ((cw - gaps_w) / aspect_sum).max(64.0)
        } else {
            th
        };
        rows.push(finalize_row(&current, final_h, None, gap));
    }

    rows
}

fn finalize_row(
    items: &[(usize, &ImageRecord, f64)],
    height: f64,
    justify_to_width: Option<i32>,
    gap: i32,
) -> JustifiedRow {
    let h = height.round().max(64.0) as i32;
    let mut tiles: Vec<RowTile> = items
        .iter()
        .map(|(idx, rec, aspect)| RowTile {
            index: *idx,
            record: (*rec).clone(),
            width: (height * aspect).round().max(48.0) as i32,
        })
        .collect();

    if let Some(target_w) = justify_to_width {
        let used: i32 = tiles.iter().map(|t| t.width).sum::<i32>()
            + gap * (tiles.len().saturating_sub(1) as i32);
        let diff = target_w - used;
        if let Some(last) = tiles.last_mut()
            && diff.abs() <= 32
        {
            last.width = (last.width + diff).max(48);
        }
    }

    JustifiedRow { height: h, tiles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_rec(id: i64, width: u32, height: u32) -> ImageRecord {
        ImageRecord {
            id,
            channel_id: 1,
            channel_slug: "ynu".to_string(),
            filename: format!("ynu_{id:04}.png"),
            rel_path: format!("media/ynu/ynu_{id:04}.png"),
            thumb_path: format!("thumbs/ynu/ynu_{id:04}.webp"),
            mime_type: "image/png".to_string(),
            width,
            height,
            size_bytes: 1000,
            thumb_bytes: 100,
            created_at: 1000,
        }
    }

    #[test]
    fn justified_rows_preserve_aspect_ratios_and_never_overflow_width() {
        let recs = vec![
            fake_rec(1, 736, 1089),
            fake_rec(2, 1920, 1080),
            fake_rec(3, 959, 959),
            fake_rec(4, 1920, 1080),
            fake_rec(5, 600, 1000),
        ];

        let rows = pack_justified_rows(&recs, 0, 900, 180, 10);
        assert!(rows.len() >= 2);

        for row in &rows {
            let total_w: i32 = row.tiles.iter().map(|t| t.width).sum::<i32>()
                + 10 * (row.tiles.len().saturating_sub(1) as i32);
            assert!(total_w <= 900, "row width {total_w} exceeded 900");
        }
        assert!(rows[0].tiles[1].width > rows[0].tiles[0].width * 2);
        assert!(target_row_height(4, 1440) > target_row_height(4, 760));
    }
}
