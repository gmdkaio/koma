use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, glib};

use crate::store::ImageRecord;
use crate::ui::media_util::{ThumbCache, load_texture, short_format};

pub struct LightboxView {
    pub root_widget: gtk4::Box,
    title_label: gtk4::Label,
    meta_pill: gtk4::Label,
    zoom_btn: gtk4::Button,
    picture: gtk4::Picture,
    filmstrip_box: gtk4::Box,
    records: Rc<RefCell<Vec<ImageRecord>>>,
    index: Rc<Cell<usize>>,
    zoom_100: Rc<Cell<bool>>,
    root_dir: PathBuf,
    thumb_cache: Rc<RefCell<ThumbCache>>,
}

impl LightboxView {
    pub fn new(
        root_dir: PathBuf,
        thumb_cache: Rc<RefCell<ThumbCache>>,
        on_close: impl Fn() + 'static,
        on_copy: impl Fn(ImageRecord) + 'static,
        on_move: impl Fn(ImageRecord) + 'static,
    ) -> Rc<Self> {
        let root_widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root_widget.add_css_class("koma-lightbox");
        root_widget.set_overflow(gtk4::Overflow::Hidden);

        let topbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        topbar.add_css_class("koma-lightbox-bar");

        let back_btn = gtk4::Button::builder()
            .icon_name("go-previous-symbolic")
            .css_classes(["flat", "koma-ghost-btn"])
            .tooltip_text("Back (Esc)")
            .build();
        let title_label = gtk4::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .css_classes(["koma-inspector-filename"])
            .build();

        let meta_pill = gtk4::Label::builder()
            .css_classes(["koma-tabular"])
            .margin_end(6)
            .build();

        let copy_btn = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .css_classes(["flat", "koma-ghost-btn"])
            .tooltip_text("Copy Image (Ctrl+C)")
            .build();
        let move_btn = gtk4::Button::builder()
            .icon_name("folder-symbolic")
            .css_classes(["flat", "koma-ghost-btn"])
            .tooltip_text("Move Channel")
            .build();
        let zoom_btn = gtk4::Button::builder()
            .label("Fit")
            .css_classes(["flat", "koma-ghost-text"])
            .tooltip_text("Toggle Fit / 100% (Z)")
            .build();

        topbar.append(&back_btn);
        topbar.append(&title_label);
        topbar.append(&meta_pill);
        topbar.append(&copy_btn);
        topbar.append(&move_btn);
        topbar.append(&zoom_btn);
        root_widget.append(&topbar);

        let picture = gtk4::Picture::new();
        picture.set_content_fit(gtk4::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_halign(gtk4::Align::Center);
        picture.set_valign(gtk4::Align::Center);

        let stage_scroll = gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .child(&picture)
            .build();

        let prev_btn = gtk4::Button::builder()
            .icon_name("go-previous-symbolic")
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Center)
            .margin_start(12)
            .css_classes(["flat", "koma-lightbox-nav"])
            .build();
        let next_btn = gtk4::Button::builder()
            .icon_name("go-next-symbolic")
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Center)
            .margin_end(12)
            .css_classes(["flat", "koma-lightbox-nav"])
            .build();

        let stage_overlay = gtk4::Overlay::new();
        stage_overlay.set_vexpand(true);
        stage_overlay.set_hexpand(true);
        stage_overlay.set_child(Some(&stage_scroll));
        stage_overlay.add_overlay(&prev_btn);
        stage_overlay.add_overlay(&next_btn);
        root_widget.append(&stage_overlay);

        let filmstrip_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        filmstrip_box.set_halign(gtk4::Align::Center);
        let filmstrip_scroll = gtk4::ScrolledWindow::builder()
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .child(&filmstrip_box)
            .css_classes(["koma-filmstrip"])
            .build();
        root_widget.append(&filmstrip_scroll);

        let view = Rc::new(Self {
            root_widget,
            title_label,
            meta_pill,
            zoom_btn,
            picture,
            filmstrip_box,
            records: Rc::new(RefCell::new(Vec::new())),
            index: Rc::new(Cell::new(0)),
            zoom_100: Rc::new(Cell::new(false)),
            root_dir,
            thumb_cache,
        });

        let on_close = Rc::new(on_close);
        let c1 = on_close.clone();
        back_btn.connect_clicked(move |_| c1());

        let v_prev = view.clone();
        prev_btn.connect_clicked(move |_| v_prev.step(-1));
        let v_next = view.clone();
        next_btn.connect_clicked(move |_| v_next.step(1));

        let v_zoom = view.clone();
        view.zoom_btn.connect_clicked(move |_| v_zoom.toggle_zoom());

        let v_copy = view.clone();
        let on_copy = Rc::new(on_copy);
        let copy_cb = on_copy.clone();
        copy_btn.connect_clicked(move |_| {
            if let Some(rec) = v_copy.current_record() {
                copy_cb(rec);
            }
        });

        let v_move = view.clone();
        move_btn.connect_clicked(move |_| {
            if let Some(rec) = v_move.current_record() {
                on_move(rec);
            }
        });

        let key_ctrl = gtk4::EventControllerKey::new();
        let v_key = view.clone();
        let close_key = on_close.clone();
        let copy_key = on_copy.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| match key {
            gdk::Key::Escape => {
                close_key();
                glib::Propagation::Stop
            }
            gdk::Key::Left => {
                v_key.step(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Right => {
                v_key.step(1);
                glib::Propagation::Stop
            }
            gdk::Key::z | gdk::Key::Z => {
                v_key.toggle_zoom();
                glib::Propagation::Stop
            }
            gdk::Key::c | gdk::Key::C if mods.contains(gdk::ModifierType::CONTROL_MASK) => {
                if let Some(rec) = v_key.current_record() {
                    copy_key(rec);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        view.root_widget.add_controller(key_ctrl);

        view
    }

    pub fn open(self: &Rc<Self>, records: Vec<ImageRecord>, index: usize) {
        if records.is_empty() {
            return;
        }
        let clamped = index.min(records.len() - 1);
        *self.records.borrow_mut() = records;
        self.index.set(clamped);
        self.zoom_100.set(false);
        self.render_current();
        self.root_widget.grab_focus();
    }

    pub fn current_record(&self) -> Option<ImageRecord> {
        self.records.borrow().get(self.index.get()).cloned()
    }

    pub fn step(self: &Rc<Self>, delta: isize) {
        let len = self.records.borrow().len();
        if len == 0 {
            return;
        }
        let cur = self.index.get() as isize;
        let next = (cur + delta).clamp(0, (len - 1) as isize) as usize;
        if next != self.index.get() {
            self.index.set(next);
            self.zoom_100.set(false);
            self.render_current();
        }
    }

    pub fn toggle_zoom(&self) {
        let next = !self.zoom_100.get();
        self.zoom_100.set(next);
        self.apply_zoom();
    }

    fn apply_zoom(&self) {
        if let Some(rec) = self.current_record() {
            if self.zoom_100.get() {
                self.zoom_btn.set_label("100%");
                self.picture.set_can_shrink(false);
                self.picture
                    .set_size_request(rec.width as i32, rec.height as i32);
            } else {
                self.zoom_btn.set_label("Fit");
                self.picture.set_can_shrink(true);
                self.picture.set_size_request(-1, -1);
            }
        }
    }

    fn render_current(self: &Rc<Self>) {
        let Some(rec) = self.current_record() else {
            return;
        };

        self.title_label
            .set_label(&format!("# {} / {}", rec.channel_slug, rec.filename));
        self.meta_pill.set_label(&format!(
            "{} × {} · {} · Local",
            rec.width,
            rec.height,
            short_format(&rec.mime_type)
        ));

        let full_abs = self.root_dir.join(&rec.rel_path);
        let tex = load_texture(&full_abs);
        self.picture.set_paintable(tex.as_ref());
        self.apply_zoom();

        // Render a bounded window of up to 21 filmstrip thumbnails around the active index.
        while let Some(child) = self.filmstrip_box.first_child() {
            self.filmstrip_box.remove(&child);
        }

        let recs = self.records.borrow();
        let active = self.index.get();
        let start = active.saturating_sub(10);
        let end = (active + 11).min(recs.len());

        for idx in start..end {
            let item_rec = &recs[idx];
            let thumb = gtk4::Picture::new();
            thumb.set_content_fit(gtk4::ContentFit::Cover);
            thumb.set_size_request(56, 44);
            let thumb_abs = self.root_dir.join(&item_rec.thumb_path);
            let tex = self.thumb_cache.borrow_mut().get_or_load(&thumb_abs);
            thumb.set_paintable(tex.as_ref());

            let btn = gtk4::Button::builder()
                .child(&thumb)
                .css_classes(["flat", "koma-filmstrip-item"])
                .build();
            if idx == active {
                btn.add_css_class("koma-filmstrip-active");
            }
            let v_jump = self.clone();
            btn.connect_clicked(move |_| {
                v_jump.index.set(idx);
                v_jump.zoom_100.set(false);
                v_jump.render_current();
            });
            self.filmstrip_box.append(&btn);
        }
    }
}
