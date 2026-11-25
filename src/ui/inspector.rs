use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, glib};

use crate::store::{Channel, ImageRecord};
use crate::ui::media_util::{ThumbCache, format_bytes, format_timestamp};

pub struct InspectorPane {
    pub root_widget: gtk4::Box,
    content_box: gtk4::Box,
    empty_label: gtk4::Label,
    preview: gtk4::Picture,
    filename_label: gtk4::Label,
    filename_entry: gtk4::Entry,
    filename_stack: gtk4::Stack,
    open_btn: gtk4::Button,
    channel_val: gtk4::Label,
    size_val: gtk4::Label,
    added_val: gtk4::Label,
    storage_val: gtk4::Label,
    quick_move_box: gtk4::FlowBox,
    current: Rc<RefCell<Option<ImageRecord>>>,
    root_dir: PathBuf,
    thumb_cache: Rc<RefCell<ThumbCache>>,
    on_quick_move: Rc<RefCell<Box<dyn Fn(i64, i64)>>>,
}

impl InspectorPane {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root_dir: PathBuf,
        thumb_cache: Rc<RefCell<ThumbCache>>,
        on_close: impl Fn() + 'static,
        on_copy: impl Fn(ImageRecord) + 'static,
        on_open_full: impl Fn(ImageRecord) + 'static,
        on_move_dialog: impl Fn(ImageRecord) + 'static,
        on_rename: impl Fn(i64, String) + 'static,
        on_quick_move: impl Fn(i64, i64) + 'static,
        on_delete: impl Fn(ImageRecord) + 'static,
    ) -> Rc<Self> {
        let root_widget = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        root_widget.add_css_class("koma-inspector");
        root_widget.set_size_request(252, -1);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let title = gtk4::Label::builder()
            .label("INSPECTOR")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["koma-section-label"])
            .build();
        let close_btn = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .css_classes(["flat", "koma-ghost-btn"])
            .tooltip_text("Close Inspector")
            .build();
        close_btn.connect_clicked(move |_| on_close());
        header.append(&title);
        header.append(&close_btn);
        root_widget.append(&header);

        let empty_label = gtk4::Label::builder()
            .label("Select an image to inspect details.")
            .wrap(true)
            .vexpand(true)
            .valign(gtk4::Align::Center)
            .css_classes(["dim-label"])
            .build();

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        content_box.set_visible(false);

        let preview = gtk4::Picture::new();
        preview.set_content_fit(gtk4::ContentFit::Contain);
        preview.set_can_shrink(true);
        preview.set_hexpand(false);
        preview.set_vexpand(false);

        let preview_frame = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        preview_frame.add_css_class("koma-inspector-preview");
        preview_frame.set_overflow(gtk4::Overflow::Hidden);
        preview_frame.set_halign(gtk4::Align::Center);
        preview_frame.append(&preview);
        content_box.append(&preview_frame);

        let filename_label = gtk4::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .css_classes(["koma-inspector-filename"])
            .build();
        let edit_btn = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .css_classes(["flat", "koma-ghost-btn"])
            .tooltip_text("Rename file")
            .build();
        let view_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        view_row.append(&filename_label);
        view_row.append(&edit_btn);

        let filename_entry = gtk4::Entry::builder()
            .hexpand(true)
            .css_classes(["koma-inline-entry"])
            .build();
        let filename_stack = gtk4::Stack::new();
        filename_stack.add_named(&view_row, Some("view"));
        filename_stack.add_named(&filename_entry, Some("edit"));
        content_box.append(&filename_stack);

        // Compact 2-column primary action bar + subtle Move row (replaces 3 stacked grey slabs).
        let primary_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        primary_row.set_homogeneous(true);

        let copy_inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        copy_inner.set_halign(gtk4::Align::Center);
        let copy_ico = gtk4::Image::from_icon_name("edit-copy-symbolic");
        copy_ico.set_pixel_size(13);
        copy_inner.append(&copy_ico);
        copy_inner.append(&gtk4::Label::new(Some("Copy")));
        let copy_btn = gtk4::Button::builder()
            .child(&copy_inner)
            .hexpand(true)
            .css_classes(["flat", "koma-accent-btn"])
            .build();

        let open_inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        open_inner.set_halign(gtk4::Align::Center);
        let open_ico = gtk4::Image::from_icon_name("view-fullscreen-symbolic");
        open_ico.set_pixel_size(13);
        open_inner.append(&open_ico);
        open_inner.append(&gtk4::Label::new(Some("Open")));
        let open_btn = gtk4::Button::builder()
            .child(&open_inner)
            .hexpand(true)
            .css_classes(["flat", "koma-action-btn"])
            .build();

        primary_row.append(&copy_btn);
        primary_row.append(&open_btn);

        let move_btn = gtk4::Button::builder()
            .label("Move to channel...")
            .hexpand(true)
            .css_classes(["flat", "koma-action-subtle"])
            .build();
        let delete_btn = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete image")
            .css_classes(["flat", "koma-action-subtle", "destructive-action"])
            .build();

        let secondary_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        secondary_row.append(&move_btn);
        secondary_row.append(&delete_btn);

        let actions = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        actions.append(&primary_row);
        actions.append(&secondary_row);
        content_box.append(&actions);

        let meta_grid = gtk4::Grid::builder()
            .column_spacing(10)
            .row_spacing(6)
            .css_classes(["koma-meta-card"])
            .build();
        let channel_val = add_meta_row(&meta_grid, 0, "Channel");
        let size_val = add_meta_row(&meta_grid, 1, "Size");
        let added_val = add_meta_row(&meta_grid, 2, "Added");
        let storage_val = add_meta_row(&meta_grid, 3, "Path");
        content_box.append(&meta_grid);

        let qm_label = gtk4::Label::builder()
            .label("QUICK MOVE")
            .xalign(0.0)
            .margin_top(4)
            .css_classes(["koma-section-label"])
            .build();
        let quick_move_box = gtk4::FlowBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .column_spacing(5)
            .row_spacing(5)
            .max_children_per_line(3)
            .css_classes(["koma-quick-move"])
            .build();
        content_box.append(&qm_label);
        content_box.append(&quick_move_box);

        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(&content_box)
            .build();

        root_widget.append(&empty_label);
        root_widget.append(&scroll);

        let current: Rc<RefCell<Option<ImageRecord>>> = Rc::new(RefCell::new(None));
        let on_rename = Rc::new(on_rename);

        let cur_edit = current.clone();
        let stack_edit = filename_stack.clone();
        let entry_edit = filename_entry.clone();
        edit_btn.connect_clicked(move |_| {
            let rec = cur_edit.borrow().clone();
            if let Some(rec) = rec {
                entry_edit.set_text(&rec.filename);
                stack_edit.set_visible_child_name("edit");
                entry_edit.grab_focus();
            }
        });

        let cur_act = current.clone();
        let stack_act = filename_stack.clone();
        let rename_cb = on_rename.clone();
        filename_entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            stack_act.set_visible_child_name("view");
            let id = cur_act.borrow().as_ref().map(|r| r.id);
            if let Some(id) = id {
                rename_cb(id, text);
            }
        });

        let key_ctrl = gtk4::EventControllerKey::new();
        let stack_esc = filename_stack.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                stack_esc.set_visible_child_name("view");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        filename_entry.add_controller(key_ctrl);

        let cur_copy = current.clone();
        copy_btn.connect_clicked(move |_| {
            let rec = cur_copy.borrow().clone();
            if let Some(rec) = rec {
                on_copy(rec);
            }
        });

        let cur_open = current.clone();
        open_btn.connect_clicked(move |_| {
            let rec = cur_open.borrow().clone();
            if let Some(rec) = rec {
                on_open_full(rec);
            }
        });

        let cur_move = current.clone();
        move_btn.connect_clicked(move |_| {
            let rec = cur_move.borrow().clone();
            if let Some(rec) = rec {
                on_move_dialog(rec);
            }
        });

        let cur_del = current.clone();
        delete_btn.connect_clicked(move |_| {
            let rec = cur_del.borrow().clone();
            if let Some(rec) = rec {
                on_delete(rec);
            }
        });

        Rc::new(Self {
            root_widget,
            content_box,
            empty_label,
            preview,
            filename_label,
            filename_entry,
            filename_stack,
            open_btn,
            channel_val,
            size_val,
            added_val,
            storage_val,
            quick_move_box,
            current,
            root_dir,
            thumb_cache,
            on_quick_move: Rc::new(RefCell::new(Box::new(on_quick_move))),
        })
    }

    pub fn set_lightbox_mode(&self, in_lightbox: bool) {
        self.open_btn.set_visible(!in_lightbox);
    }

    pub fn begin_inline_rename(&self) {
        let rec = self.current.borrow().clone();
        let Some(rec) = rec else { return };
        self.filename_entry.set_text(&rec.filename);
        self.filename_stack.set_visible_child_name("edit");
        self.filename_entry.grab_focus();
        let stem_end = rec
            .filename
            .rfind('.')
            .filter(|&i| i > 0)
            .unwrap_or(rec.filename.len()) as i32;
        self.filename_entry.select_region(0, stem_end);
    }

    pub fn current_id(&self) -> Option<i64> {
        self.current.borrow().as_ref().map(|r| r.id)
    }

    pub fn set_image(&self, rec: Option<ImageRecord>, channels: &[Channel]) {
        *self.current.borrow_mut() = rec.clone();
        self.filename_stack.set_visible_child_name("view");

        let Some(rec) = rec else {
            self.content_box.set_visible(false);
            self.empty_label.set_visible(true);
            self.preview.set_paintable(gdk::Paintable::NONE);
            return;
        };

        self.empty_label.set_visible(false);
        self.content_box.set_visible(true);

        let thumb_abs = self.root_dir.join(&rec.thumb_path);
        let tex = self.thumb_cache.borrow_mut().get_or_load(&thumb_abs);
        let max_w = 236.0f64;
        let aspect = if rec.height > 0 && rec.width > 0 {
            (rec.width as f64) / (rec.height as f64)
        } else {
            1.0
        };
        let (pw, ph) = if aspect >= 1.0 {
            let h = (max_w / aspect).clamp(96.0, 240.0);
            (max_w.round() as i32, h.round() as i32)
        } else {
            let h = (max_w / aspect).clamp(120.0, 260.0);
            let w = (h * aspect).clamp(96.0, max_w);
            (w.round() as i32, h.round() as i32)
        };
        self.preview.set_size_request(pw, ph);
        self.preview.set_paintable(tex.as_ref());

        self.filename_label.set_label(&rec.filename);
        self.filename_entry.set_text(&rec.filename);
        self.channel_val.set_label(&format!("# {}", rec.channel_slug));
        self.size_val.set_label(&format!(
            "{} × {} · {}",
            rec.width,
            rec.height,
            format_bytes(rec.size_bytes)
        ));
        self.added_val.set_label(&format_timestamp(rec.created_at));
        self.storage_val
            .set_label(&format!("koma/{}/", rec.channel_slug));

        while let Some(child) = self.quick_move_box.first_child() {
            self.quick_move_box.remove(&child);
        }

        let image_id = rec.id;
        for ch in channels
            .iter()
            .filter(|c| c.id != rec.channel_id)
            .take(5)
        {
            let chip = gtk4::Button::builder()
                .label(format!("#{}", ch.slug))
                .css_classes(["flat", "koma-chip"])
                .build();
            let target_id = ch.id;
            let cb = self.on_quick_move.clone();
            chip.connect_clicked(move |_| {
                (cb.borrow())(image_id, target_id);
            });
            self.quick_move_box.insert(&chip, -1);
            if let Some(child) = chip.parent() {
                child.set_focusable(false);
            }
        }
    }
}

fn add_meta_row(grid: &gtk4::Grid, row: i32, key: &str) -> gtk4::Label {
    let k = gtk4::Label::builder()
        .label(key)
        .xalign(0.0)
        .css_classes(["koma-meta-key"])
        .build();
    let v = gtk4::Label::builder()
        .xalign(1.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .css_classes(["koma-meta-val"])
        .build();
    grid.attach(&k, 0, row, 1, 1);
    grid.attach(&v, 1, row, 1, 1);
    v
}
