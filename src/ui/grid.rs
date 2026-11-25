use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

use crate::store::ImageRecord;
use crate::ui::media_util::AsyncThumbLoader;

const ROW_GAP: i32 = 6;
/// Horizontal margin (14px * 2) + vertical scrollbar clearance (14px).
const CANVAS_PAD_X: i32 = 42;
const WHEEL_STEP_PX: f64 = 148.0;
const TOP_LIST_PAD: f64 = 54.0;
const HEADER_ROW_HEIGHT: f64 = 38.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
struct RowTile {
    index: usize,
    width: i32,
}

#[derive(Clone, Debug)]
struct JustifiedRow {
    height: i32,
    tiles: Vec<RowTile>,
}

#[derive(Clone, Debug)]
enum ListRowItem {
    Header {
        label: String,
        count: usize,
        first: bool,
    },
    Images(JustifiedRow),
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RowObject {
        pub(super) item: RefCell<Option<ListRowItem>>,
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
    fn new(item: ListRowItem) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().item.replace(Some(item));
        obj
    }

    fn set_item(&self, item: ListRowItem) {
        self.imp().item.replace(Some(item));
    }

    fn item(&self) -> ListRowItem {
        self.imp()
            .item
            .borrow()
            .clone()
            .expect("RowObject initialized with item")
    }
}

struct SmoothScrollState {
    target: f64,
    active: bool,
    last_frame_us: i64,
}

pub struct GridCanvas {
    pub stack: gtk4::Stack,
    row_store: gio::ListStore,
    list_view: gtk4::ListView,
    grid_scroll: gtk4::ScrolledWindow,
    live_items: Rc<RefCell<Vec<glib::WeakRef<gtk4::ListItem>>>>,
    packed_items: Rc<RefCell<Vec<ListRowItem>>>,
    smooth_scroll: Rc<RefCell<SmoothScrollState>>,
    records: Rc<RefCell<Vec<ImageRecord>>>,
    selected_ids: Rc<RefCell<HashSet<i64>>>,
    last_clicked_id: Rc<Cell<Option<i64>>>,
    /// Zoom level 1..=7 (1 = zoomed out compact rows, 7 = zoomed in large rows).
    zoom_level: Rc<Cell<i32>>,
    viewport_width: Rc<Cell<i32>>,
    latest_raw_width: Rc<Cell<i32>>,
    viewport_initialized: Rc<Cell<bool>>,
    pending_resize: Rc<RefCell<Option<glib::SourceId>>>,
    /// Suppresses intermediate resize ticks while OverlaySplitView plays its spring animation.
    resize_freeze_until: Rc<Cell<Option<Instant>>>,
    panel_anim_gen: Rc<Cell<u64>>,
    select_mode: Rc<Cell<bool>>,
    show_channel_badges: Rc<Cell<bool>>,
    timeline_mode: Rc<Cell<bool>>,
    chronological_sort: Rc<Cell<bool>>,
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
        let live_items: Rc<RefCell<Vec<glib::WeakRef<gtk4::ListItem>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let packed_items: Rc<RefCell<Vec<ListRowItem>>> = Rc::new(RefCell::new(Vec::new()));
        let zoom_level = Rc::new(Cell::new(4));
        let viewport_width = Rc::new(Cell::new(920));
        let latest_raw_width = Rc::new(Cell::new(962));
        let viewport_initialized = Rc::new(Cell::new(false));
        let pending_resize: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let resize_freeze_until = Rc::new(Cell::new(None));
        let panel_anim_gen = Rc::new(Cell::new(0));
        let select_mode = Rc::new(Cell::new(false));
        let show_channel_badges = Rc::new(Cell::new(false));
        let timeline_mode = Rc::new(Cell::new(false));
        let chronological_sort = Rc::new(Cell::new(true));
        let records: Rc<RefCell<Vec<ImageRecord>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_ids: Rc<RefCell<HashSet<i64>>> = Rc::new(RefCell::new(HashSet::new()));
        let last_clicked_id = Rc::new(Cell::new(None));
        let smooth_scroll = Rc::new(RefCell::new(SmoothScrollState {
            target: 0.0,
            active: false,
            last_frame_us: 0,
        }));

        let on_selection_change: Rc<RefCell<Box<dyn Fn(Vec<ImageRecord>, Option<ImageRecord>)>>> =
            Rc::new(RefCell::new(Box::new(on_selection_change)));
        let on_activate_image: Rc<RefCell<Box<dyn Fn(ImageRecord)>>> =
            Rc::new(RefCell::new(Box::new(on_activate_image)));
        let on_jump_channel: Rc<RefCell<Box<dyn Fn(i64)>>> =
            Rc::new(RefCell::new(Box::new(on_jump_channel)));

        let factory = gtk4::SignalListItemFactory::new();
        let live_setup = live_items.clone();
        factory.connect_setup(move |_, obj| {
            let item = obj
                .downcast_ref::<gtk4::ListItem>()
                .expect("ListItem in setup");
            item.set_activatable(false);
            item.set_selectable(false);
            item.set_focusable(false);
            let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, ROW_GAP);
            row_box.set_hexpand(true);
            item.set_child(Some(&row_box));
            live_setup.borrow_mut().push(item.downgrade());
        });

        let list_view = gtk4::ListView::builder()
            .model(&no_sel)
            .factory(&factory)
            .single_click_activate(false)
            .focusable(true)
            .can_focus(true)
            .css_classes(["koma-gallery-list"])
            .build();

        let grid_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::External)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .propagate_natural_width(false)
            .min_content_width(220)
            .vexpand(true)
            .hexpand(true)
            .child(&list_view)
            .build();

        attach_smooth_wheel(&grid_scroll, smooth_scroll.clone());

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
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.add_named(&grid_scroll, Some("list"));
        stack.add_named(&empty_page, Some("empty"));

        let canvas = Rc::new(Self {
            stack,
            row_store,
            list_view,
            grid_scroll,
            live_items,
            packed_items,
            smooth_scroll,
            records,
            selected_ids,
            last_clicked_id,
            zoom_level,
            viewport_width,
            latest_raw_width,
            viewport_initialized,
            pending_resize,
            resize_freeze_until,
            panel_anim_gen,
            select_mode,
            show_channel_badges,
            timeline_mode,
            chronological_sort,
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
            c_bind.populate_row_item(
                &row_box,
                &row_obj.item(),
                c_bind.show_channel_badges.get(),
                false,
            );
        });

        factory.connect_unbind(|_, obj| {
            let item = obj
                .downcast_ref::<gtk4::ListItem>()
                .expect("ListItem in unbind");
            if let Some(row_box) = item.child().and_downcast::<gtk4::Box>() {
                row_box.set_widget_name("");
                while let Some(child) = row_box.first_child() {
                    if let Some(card) = child.downcast_ref::<gtk4::Box>()
                        && let Some(overlay) = card.first_child().and_downcast::<gtk4::Overlay>()
                        && let Some(pic_wrap) = overlay.child().and_downcast::<gtk4::Box>()
                        && let Some(picture) = pic_wrap.first_child().and_downcast::<gtk4::Picture>()
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

    /// Subtle glide-up + fade when switching between Home / channels / Grid / Timeline.
    fn pulse_panel_transition(&self) {
        let next_gen = self.panel_anim_gen.get().wrapping_add(1);
        self.panel_anim_gen.set(next_gen);
        self.list_view.add_css_class("koma-panel-enter");
        let lv = self.list_view.clone();
        let gen_ref = self.panel_anim_gen.clone();
        glib::timeout_add_local_once(Duration::from_millis(18), move || {
            if gen_ref.get() == next_gen {
                lv.remove_css_class("koma-panel-enter");
            }
        });
    }

    fn current_raw_width(&self) -> i32 {
        let w = self.grid_scroll.width();
        if w > 0 {
            w
        } else {
            self.latest_raw_width.get()
        }
    }

    /// Called right when a sidebar (Inspector or left sidebar) begins sliding so the spring
    /// animation runs at full 60/120fps with zero mid-animation repacks, then settles cleanly once.
    pub fn notify_sidebar_animating(self: &Rc<Self>, delta_px: i32) {
        if let Some(prev) = self.pending_resize.borrow_mut().take() {
            prev.remove();
        }
        // When opening Inspector (shrinking gallery), pre-pack once to final width immediately
        // so the sidebar slides cleanly into empty space at 120fps.
        if delta_px < 0 && self.viewport_initialized.get() {
            let target_usable = (self.current_raw_width() + delta_px - CANVAS_PAD_X).max(260);
            self.viewport_width.set(target_usable);
            if !self.records.borrow().is_empty() {
                let c = self.clone();
                glib::idle_add_local_once(move || c.repack_rows_in_place(true));
            }
        }
        self.resize_freeze_until
            .set(Some(Instant::now() + Duration::from_millis(420)));
        let c = self.clone();
        let src = glib::timeout_add_local_once(Duration::from_millis(430), move || {
            *c.pending_resize.borrow_mut() = None;
            c.resize_freeze_until.set(None);
            let final_usable = (c.current_raw_width() - CANVAS_PAD_X).max(260);
            if (c.viewport_width.get() - final_usable).abs() >= 4 {
                c.viewport_width.set(final_usable);
                if !c.records.borrow().is_empty() {
                    c.repack_rows_in_place(true);
                }
            }
        });
        *self.pending_resize.borrow_mut() = Some(src);
    }

    fn populate_row_item(
        self: &Rc<Self>,
        row_box: &gtk4::Box,
        item: &ListRowItem,
        with_channel_badge: bool,
        allow_reuse: bool,
    ) {
        match item {
            ListRowItem::Header {
                label,
                count,
                first,
            } => {
                while let Some(child) = row_box.first_child() {
                    row_box.remove(&child);
                }
                row_box.set_widget_name("koma-hdr");
                row_box.set_halign(gtk4::Align::Fill);
                row_box.set_spacing(10);
                row_box.set_margin_top(if *first { 2 } else { 14 });
                row_box.set_margin_bottom(4);

                let title = gtk4::Label::builder()
                    .label(label.to_uppercase())
                    .css_classes(["koma-timeline-title"])
                    .build();
                let count_lbl = gtk4::Label::builder()
                    .label(count.to_string())
                    .css_classes(["koma-tabular"])
                    .build();
                let rule = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                rule.add_css_class("koma-timeline-rule");
                rule.set_hexpand(true);
                rule.set_valign(gtk4::Align::Center);
                row_box.append(&title);
                row_box.append(&count_lbl);
                row_box.append(&rule);
            }
            ListRowItem::Images(row) => {
                let recs = self.records.borrow();
                if allow_reuse && row_box.widget_name() == "koma-imgs" {
                    let mut existing = Vec::new();
                    let mut c = row_box.first_child();
                    while let Some(w) = c {
                        existing.push(w.clone());
                        c = w.next_sibling();
                    }
                    let same_tiles = existing.len() == row.tiles.len()
                        && existing.iter().zip(row.tiles.iter()).all(|(w, t)| {
                            recs.get(t.index)
                                .is_some_and(|r| w.widget_name() == r.id.to_string().as_str())
                        });
                    if same_tiles {
                        let ids = self.selected_ids.borrow();
                        let show_checks = self.select_mode.get() || ids.len() > 1;
                        for (w, tile) in existing.iter().zip(row.tiles.iter()) {
                            if let Some(overlay) = w.first_child().and_downcast::<gtk4::Overlay>() {
                                overlay.set_size_request(tile.width, row.height);
                            }
                        }
                        Self::update_row_selection(row_box, &ids, show_checks);
                        return;
                    }
                }

                while let Some(child) = row_box.first_child() {
                    row_box.remove(&child);
                }
                row_box.set_widget_name("koma-imgs");
                row_box.set_halign(gtk4::Align::Start);
                row_box.set_spacing(ROW_GAP);
                row_box.set_margin_top(0);
                row_box.set_margin_bottom(0);

                let selected_snapshot = self.selected_ids.borrow().clone();
                let show_checks = self.select_mode.get() || selected_snapshot.len() > 1;

                for tile in &row.tiles {
                    let Some(rec) = recs.get(tile.index) else {
                        continue;
                    };
                    let is_sel = selected_snapshot.contains(&rec.id);
                    let card = self.build_tile_card(
                        rec,
                        tile.width,
                        row.height,
                        is_sel,
                        show_checks,
                        with_channel_badge,
                    );
                    row_box.append(&card);
                }
            }
        }
    }

    fn build_tile_card(
        self: &Rc<Self>,
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

        // Scales slightly inward on selection (Apple Photos / Eagle style) inside the accent matte
        // so selection is crisp on pure-white manga panels and dark scenes alike, with zero shadow.
        let pic_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        pic_wrap.add_css_class("koma-tile-media");
        pic_wrap.set_overflow(gtk4::Overflow::Hidden);
        pic_wrap.set_hexpand(true);
        pic_wrap.set_vexpand(true);
        pic_wrap.append(&picture);

        let check = gtk4::Label::builder()
            .label("✓")
            .name("koma-check")
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .visible(is_sel && show_check)
            .css_classes(["koma-check-badge"])
            .build();

        let overlay = gtk4::Overlay::new();
        overlay.set_size_request(tile_w, tile_h);
        overlay.set_overflow(gtk4::Overflow::Hidden);
        overlay.add_css_class("koma-tile-frame");
        overlay.set_child(Some(&pic_wrap));
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

        // Look up the record dynamically by ID so deletes/sorts never leave stale indices.
        let click = gtk4::GestureClick::new();
        let c_click = self.clone();
        let rec_id = rec.id;
        click.connect_released(move |gesture, n_press, _, _| {
            c_click.list_view.grab_focus();
            let Some((idx, live_rec)) = c_click
                .records
                .borrow()
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == rec_id)
                .map(|(i, r)| (i, r.clone()))
            else {
                return;
            };
            if n_press >= 2 {
                c_click.select_single(&live_rec);
                (c_click.on_activate_image.borrow())(live_rec);
                return;
            }
            let mods = gesture.current_event_state();
            c_click.handle_tile_click(idx, &live_rec, mods);
        });
        overlay.add_controller(click);
        card
    }

    fn select_single(self: &Rc<Self>, rec: &ImageRecord) {
        {
            let mut ids = self.selected_ids.borrow_mut();
            ids.clear();
            ids.insert(rec.id);
        }
        self.last_clicked_id.set(Some(rec.id));
        self.refresh_selection_visuals();
        (self.on_selection_change.borrow())(vec![rec.clone()], Some(rec.clone()));
    }

    /// Move selection spatially with arrow keys and smoothly scroll the selected tile into view.
    pub fn move_selection(self: &Rc<Self>, dir: NavDir, extend: bool) -> bool {
        let recs = self.records.borrow().clone();
        if recs.is_empty() {
            return false;
        }

        let cur_idx = self
            .last_clicked_id
            .get()
            .and_then(|id| recs.iter().position(|r| r.id == id));

        let next = match cur_idx {
            None => 0,
            Some(cur) => match dir {
                NavDir::Left => cur.saturating_sub(1),
                NavDir::Right => (cur + 1).min(recs.len() - 1),
                NavDir::Up => self.spatial_vertical_step(cur, -1).unwrap_or(cur),
                NavDir::Down => self.spatial_vertical_step(cur, 1).unwrap_or(cur),
            },
        };

        let target_rec = recs[next].clone();
        {
            let mut ids = self.selected_ids.borrow_mut();
            if !extend && !self.select_mode.get() {
                ids.clear();
            }
            ids.insert(target_rec.id);
        }
        self.last_clicked_id.set(Some(target_rec.id));

        let selected_list: Vec<ImageRecord> = {
            let ids = self.selected_ids.borrow();
            recs.iter().filter(|r| ids.contains(&r.id)).cloned().collect()
        };
        self.refresh_selection_visuals();
        self.scroll_index_into_view(next);
        (self.on_selection_change.borrow())(selected_list, Some(target_rec));
        true
    }

    fn spatial_vertical_step(&self, current_idx: usize, step: isize) -> Option<usize> {
        let items = self.packed_items.borrow();
        let mut cur_row_pos = None;
        let mut cur_center_x = 0.0f64;

        for (r_idx, item) in items.iter().enumerate() {
            if let ListRowItem::Images(row) = item {
                let mut x = 0.0f64;
                for tile in &row.tiles {
                    let w = tile.width as f64;
                    if tile.index == current_idx {
                        cur_row_pos = Some(r_idx);
                        cur_center_x = x + w * 0.5;
                        break;
                    }
                    x += w + (ROW_GAP as f64);
                }
                if cur_row_pos.is_some() {
                    break;
                }
            }
        }

        let start = cur_row_pos? as isize;
        let mut probe = start + step;
        while probe >= 0 && (probe as usize) < items.len() {
            if let ListRowItem::Images(row) = &items[probe as usize] {
                let mut best_idx = row.tiles.first()?.index;
                let mut best_dist = f64::INFINITY;
                let mut x = 0.0f64;
                for tile in &row.tiles {
                    let w = tile.width as f64;
                    let center = x + w * 0.5;
                    let dist = (center - cur_center_x).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        best_idx = tile.index;
                    }
                    x += w + (ROW_GAP as f64);
                }
                return Some(best_idx);
            }
            probe += step;
        }
        None
    }

    fn scroll_index_into_view(&self, target_idx: usize) {
        let items = self.packed_items.borrow();
        let mut y = TOP_LIST_PAD;
        let mut found = None;
        for item in items.iter() {
            match item {
                ListRowItem::Header { first, .. } => {
                    y += HEADER_ROW_HEIGHT + if *first { 0.0 } else { 14.0 };
                }
                ListRowItem::Images(row) => {
                    let h = (row.height + ROW_GAP) as f64;
                    if row.tiles.iter().any(|t| t.index == target_idx) {
                        found = Some((y, y + h));
                        break;
                    }
                    y += h;
                }
            }
        }
        drop(items);

        let Some((row_top, row_bottom)) = found else {
            return;
        };
        let vadj = self.grid_scroll.vadjustment();
        let cur = vadj.value();
        let page = vadj.page_size();
        let max = (vadj.upper() - page).max(0.0);

        let top_pad = 64.0;
        let bot_pad = 20.0;
        let desired = if row_top < cur + top_pad {
            Some((row_top - top_pad).clamp(0.0, max))
        } else if row_bottom > cur + page - bot_pad {
            Some((row_bottom - page + bot_pad).clamp(0.0, max))
        } else {
            None
        };

        if let Some(target) = desired {
            smooth_scroll_to(&self.grid_scroll, &self.smooth_scroll, target);
        }
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
            let recs = self.records.borrow();
            let anchor = self
                .last_clicked_id
                .get()
                .and_then(|aid| recs.iter().position(|r| r.id == aid));
            if shift
                && let Some(anchor) = anchor
            {
                let (lo, hi) = if anchor <= idx {
                    (anchor, idx)
                } else {
                    (idx, anchor)
                };
                if !ctrl && !self.select_mode.get() {
                    ids.clear();
                }
                for r in recs.iter().skip(lo).take(hi - lo + 1) {
                    ids.insert(r.id);
                }
            } else if multi {
                if !ids.remove(&rec.id) {
                    ids.insert(rec.id);
                }
                self.last_clicked_id.set(Some(rec.id));
            } else {
                ids.clear();
                ids.insert(rec.id);
                self.last_clicked_id.set(Some(rec.id));
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

        let mut row_widget = self.list_view.first_child();
        while let Some(rw) = row_widget {
            if let Some(row_box) = rw.first_child().and_downcast::<gtk4::Box>() {
                Self::update_row_selection(&row_box, &ids, show_checks);
            }
            row_widget = rw.next_sibling();
        }
    }

    pub fn focus_widget(&self) -> gtk4::Widget {
        self.list_view.clone().upcast()
    }

    pub fn focus_canvas(&self) {
        self.list_view.grab_focus();
    }

    fn reset_scroll(&self) {
        let mut st = self.smooth_scroll.borrow_mut();
        st.active = false;
        st.target = 0.0;
        drop(st);
        self.grid_scroll.vadjustment().set_value(0.0);
    }

    /// Never mutates widgets synchronously inside GTK's `size_allocate` pass; defers cleanly so
    /// niri window drags and sidebar spring animations stay at native monitor framerate.
    pub fn set_viewport_width(self: &Rc<Self>, width: i32) {
        self.latest_raw_width.set(width);
        let usable = (width - CANVAS_PAD_X).max(260);

        if !self.viewport_initialized.get() {
            self.viewport_width.set(usable);
            self.viewport_initialized.set(true);
            let c = self.clone();
            glib::idle_add_local_once(move || {
                if !c.records.borrow().is_empty() {
                    c.repack_rows_in_place(true);
                }
            });
            return;
        }

        if let Some(until) = self.resize_freeze_until.get()
            && Instant::now() < until
        {
            return;
        }
        if (self.viewport_width.get() - usable).abs() < 4 {
            return;
        }

        if let Some(prev) = self.pending_resize.borrow_mut().take() {
            prev.remove();
        }
        let c = self.clone();
        let src = glib::timeout_add_local_once(Duration::from_millis(65), move || {
            *c.pending_resize.borrow_mut() = None;
            let settled = (c.current_raw_width() - CANVAS_PAD_X).max(260);
            if (c.viewport_width.get() - settled).abs() < 4 {
                return;
            }
            c.viewport_width.set(settled);
            if !c.records.borrow().is_empty() {
                c.repack_rows_in_place(true);
            }
        });
        *self.pending_resize.borrow_mut() = Some(src);
    }

    pub fn set_records(
        self: &Rc<Self>,
        recs: Vec<ImageRecord>,
        view_mode: &str,
        is_home: bool,
        chronological_sort: bool,
        preserve_scroll: bool,
    ) {
        if !preserve_scroll {
            self.loader.bump_generation();
            self.selected_ids.borrow_mut().clear();
            self.last_clicked_id.set(None);
        } else {
            let valid_ids: HashSet<i64> = recs.iter().map(|r| r.id).collect();
            self.selected_ids
                .borrow_mut()
                .retain(|id| valid_ids.contains(id));
            if let Some(lid) = self.last_clicked_id.get()
                && !valid_ids.contains(&lid)
            {
                self.last_clicked_id.set(None);
            }
        }

        self.show_channel_badges.set(is_home);
        self.timeline_mode.set(view_mode == "timeline");
        self.chronological_sort.set(chronological_sort);
        *self.records.borrow_mut() = recs;

        if self.records.borrow().is_empty() {
            self.packed_items.borrow_mut().clear();
            self.row_store.remove_all();
            self.stack.set_visible_child_name("empty");
            (self.on_selection_change.borrow())(Vec::new(), None);
            return;
        }

        self.repack_rows_in_place(false);
        self.stack.set_visible_child_name("list");
        if !preserve_scroll {
            self.reset_scroll();
            self.pulse_panel_transition();
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
        let focused = selected_list
            .first()
            .cloned()
            .or_else(|| self.records.borrow().first().cloned());
        (self.on_selection_change.borrow())(selected_list, focused);
    }

    pub fn set_mode(self: &Rc<Self>, view_mode: &str) {
        let next_timeline = view_mode == "timeline";
        if self.records.borrow().is_empty() {
            self.stack.set_visible_child_name("empty");
            return;
        }
        self.timeline_mode.set(next_timeline);
        self.repack_rows_in_place(false);
        self.stack.set_visible_child_name("list");
        self.pulse_panel_transition();
    }

    pub fn set_zoom_level(self: &Rc<Self>, level: i32) {
        let clamped = level.clamp(1, 7);
        if self.zoom_level.get() == clamped {
            return;
        }
        self.zoom_level.set(clamped);
        if !self.records.borrow().is_empty() {
            self.repack_rows_in_place(true);
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
        self.last_clicked_id.set(None);
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

    fn repack_rows_in_place(self: &Rc<Self>, allow_reuse: bool) {
        let freezing = self
            .resize_freeze_until
            .get()
            .is_some_and(|until| Instant::now() < until);
        if !freezing && self.grid_scroll.width() > 0 {
            let live_usable = (self.grid_scroll.width() - CANVAS_PAD_X).max(260);
            self.viewport_width.set(live_usable);
            self.viewport_initialized.set(true);
        }
        let vw = self.viewport_width.get();
        let target_h = target_row_height(self.zoom_level.get(), vw);

        let new_items: Vec<ListRowItem> = {
            let recs = self.records.borrow();
            if self.timeline_mode.get() && self.chronological_sort.get() {
                let mut bucketer = TimelineBucketer::new();
                let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
                for (idx, rec) in recs.iter().enumerate() {
                    let bucket = bucketer.label_for(rec.created_at);
                    if let Some(last) = groups.last_mut()
                        && last.0 == bucket
                    {
                        last.1.push(idx);
                    } else {
                        groups.push((bucket, vec![idx]));
                    }
                }

                let mut items = Vec::new();
                for (g_idx, (label, indices)) in groups.into_iter().enumerate() {
                    let count = indices.len();
                    items.push(ListRowItem::Header {
                        label,
                        count,
                        first: g_idx == 0,
                    });
                    for row in pack_indices(&recs, &indices, vw, target_h, ROW_GAP) {
                        items.push(ListRowItem::Images(row));
                    }
                }
                items
            } else {
                pack_slice(&recs, vw, target_h, ROW_GAP)
                    .into_iter()
                    .map(ListRowItem::Images)
                    .collect()
            }
        };

        let old_len = self.row_store.n_items() as usize;
        let new_len = new_items.len();
        let common = old_len.min(new_len);

        for (i, item) in new_items.iter().take(common).enumerate() {
            if let Some(row_obj) = self.row_store.item(i as u32).and_downcast::<RowObject>() {
                row_obj.set_item(item.clone());
            }
        }

        if new_len > old_len {
            let added: Vec<RowObject> = new_items[old_len..]
                .iter()
                .cloned()
                .map(RowObject::new)
                .collect();
            self.row_store.splice(old_len as u32, 0, &added);
        } else if new_len < old_len {
            self.row_store.splice(
                new_len as u32,
                (old_len - new_len) as u32,
                &[] as &[RowObject],
            );
        }

        *self.packed_items.borrow_mut() = new_items;

        let with_badges = self.show_channel_badges.get();
        let live: Vec<gtk4::ListItem> = {
            let mut list = self.live_items.borrow_mut();
            list.retain(|w| w.upgrade().is_some());
            list.iter().filter_map(|w| w.upgrade()).collect()
        };
        // Update EVERY bound ListItem (both mapped on-screen and off-screen pre-buffered rows),
        // otherwise GtkListView scrolls stale narrow/old rows into view without re-calling bind.
        for list_item in live {
            if let Some(row_obj) = list_item.item().and_downcast::<RowObject>()
                && let Some(row_box) = list_item.child().and_downcast::<gtk4::Box>()
            {
                self.populate_row_item(&row_box, &row_obj.item(), with_badges, allow_reuse);
                row_box.queue_resize();
            }
        }
        self.list_view.queue_resize();
    }
}

struct TimelineBucketer {
    today_ymd: Option<(i32, i32, i32)>,
    yesterday_ymd: Option<(i32, i32, i32)>,
    month_cache: HashMap<(i32, i32), String>,
}

impl TimelineBucketer {
    fn new() -> Self {
        let now = glib::DateTime::now_local().ok();
        let today_ymd = now.as_ref().map(|d| d.ymd());
        let yesterday_ymd = now.as_ref().and_then(|d| d.add_days(-1).ok()).map(|d| d.ymd());
        Self {
            today_ymd,
            yesterday_ymd,
            month_cache: HashMap::new(),
        }
    }

    fn label_for(&mut self, unix_secs: i64) -> String {
        let Ok(dt) = glib::DateTime::from_unix_local(unix_secs) else {
            return "Earlier".to_string();
        };
        let ymd = dt.ymd();
        if self.today_ymd == Some(ymd) {
            return "Today".to_string();
        }
        if self.yesterday_ymd == Some(ymd) {
            return "Yesterday".to_string();
        }
        self.month_cache
            .entry((ymd.0, ymd.1))
            .or_insert_with(|| {
                dt.format("%B %Y")
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "Earlier".to_string())
            })
            .clone()
    }
}

fn smooth_scroll_to(
    scroll: &gtk4::ScrolledWindow,
    state: &Rc<RefCell<SmoothScrollState>>,
    target: f64,
) {
    let vadj = scroll.vadjustment();
    let max = (vadj.upper() - vadj.page_size()).max(0.0);
    let clamped = target.clamp(0.0, max);

    let should_start_tick = {
        let mut st = state.borrow_mut();
        st.target = clamped;
        if !st.active {
            st.active = true;
            st.last_frame_us = 0;
            true
        } else {
            false
        }
    };

    if should_start_tick {
        let st_tick = state.clone();
        let vadj_tick = vadj;
        scroll.add_tick_callback(move |_, clock| {
            let mut st = st_tick.borrow_mut();
            if !st.active {
                return glib::ControlFlow::Break;
            }
            let now_us = clock.frame_time();
            let dt = if st.last_frame_us > 0 {
                ((now_us - st.last_frame_us) as f64 / 1_000_000.0).clamp(0.001, 0.05)
            } else {
                0.016
            };
            st.last_frame_us = now_us;

            let cur = vadj_tick.value();
            let max = (vadj_tick.upper() - vadj_tick.page_size()).max(0.0);
            let target = st.target.clamp(0.0, max);
            let diff = target - cur;
            if diff.abs() < 0.6 {
                vadj_tick.set_value(target);
                st.active = false;
                return glib::ControlFlow::Break;
            }
            let alpha = 1.0 - (-18.0 * dt).exp();
            vadj_tick.set_value((cur + diff * alpha).clamp(0.0, max));
            glib::ControlFlow::Continue
        });
    }
}

/// Frame-synced exponential easing for discrete mouse-wheel ticks; leaves touchpad smooth scroll native.
fn attach_smooth_wheel(scroll: &gtk4::ScrolledWindow, state: Rc<RefCell<SmoothScrollState>>) {
    let ctrl = gtk4::EventControllerScroll::new(
        gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE,
    );
    ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let scroll_ref = scroll.clone();
    let state_cb = state;
    ctrl.connect_scroll(move |_, _dx, dy| {
        if dy.abs() < 0.0001 {
            return glib::Propagation::Proceed;
        }
        let vadj = scroll_ref.vadjustment();
        let cur = vadj.value();
        let base = {
            let st = state_cb.borrow();
            if st.active { st.target } else { cur }
        };
        smooth_scroll_to(&scroll_ref, &state_cb, base + dy * WHEEL_STEP_PX);
        glib::Propagation::Stop
    });

    scroll.add_controller(ctrl);
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
    let scale = ((viewport_width as f64) / 760.0).clamp(0.95, 1.44);
    (base * scale).round() as i32
}

fn pack_slice(
    records: &[ImageRecord],
    container_width: i32,
    target_height: i32,
    gap: i32,
) -> Vec<JustifiedRow> {
    let indices: Vec<usize> = (0..records.len()).collect();
    pack_indices(records, &indices, container_width, target_height, gap)
}

fn pack_indices(
    records: &[ImageRecord],
    indices: &[usize],
    container_width: i32,
    target_height: i32,
    gap: i32,
) -> Vec<JustifiedRow> {
    if indices.is_empty() || container_width <= 0 {
        return Vec::new();
    }

    let cw = container_width as f64;
    let th = target_height as f64;
    let g = gap as f64;

    let mut rows = Vec::new();
    let mut current: Vec<(usize, f64)> = Vec::new();
    let mut aspect_sum = 0.0f64;

    for &idx in indices {
        let rec = &records[idx];
        let aspect = clamped_aspect(rec.width, rec.height);
        current.push((idx, aspect));
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
        rows.push(finalize_row(&current, th, None, gap));
    }

    rows
}

fn finalize_row(
    items: &[(usize, f64)],
    row_height: f64,
    fit_width: Option<i32>,
    gap: i32,
) -> JustifiedRow {
    let h = row_height.round().max(64.0) as i32;
    let mut widths: Vec<i32> = items
        .iter()
        .map(|(_, aspect)| ((row_height * aspect).round() as i32).max(48))
        .collect();

    if let Some(target_w) = fit_width {
        let gaps = gap * (items.len().saturating_sub(1) as i32);
        let sum: i32 = widths.iter().sum::<i32>() + gaps;
        let diff = target_w - sum;
        if let Some(last) = widths.last_mut() {
            *last = (*last + diff).max(48);
        }
    }

    let tiles = items
        .iter()
        .zip(widths)
        .map(|((idx, _), width)| RowTile { index: *idx, width })
        .collect();

    JustifiedRow { height: h, tiles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_rec(id: i64, w: u32, h: u32) -> ImageRecord {
        ImageRecord {
            id,
            channel_id: 1,
            channel_slug: "rei".to_string(),
            filename: format!("rei_{id:04}.png"),
            rel_path: String::new(),
            thumb_path: String::new(),
            mime_type: "image/png".to_string(),
            width: w,
            height: h,
            size_bytes: 100,
            thumb_bytes: 50,
            created_at: 1000 + id,
        }
    }

    #[test]
    fn justified_rows_preserve_aspect_ratios_and_never_overflow_width() {
        let recs = vec![
            fake_rec(1, 1920, 1080),
            fake_rec(2, 1080, 1920),
            fake_rec(3, 1200, 1200),
            fake_rec(4, 1600, 900),
            fake_rec(5, 800, 1200),
            fake_rec(6, 1400, 1000),
        ];
        let container_w = 900;
        let rows = pack_slice(&recs, container_w, 180, ROW_GAP);
        assert!(rows.len() >= 2);

        for (i, row) in rows.iter().enumerate() {
            let total_w: i32 = row.tiles.iter().map(|t| t.width).sum::<i32>()
                + ROW_GAP * (row.tiles.len().saturating_sub(1) as i32);
            if i + 1 < rows.len() {
                assert_eq!(total_w, container_w);
            } else {
                assert!(total_w <= container_w + 48);
            }
        }
    }
}
