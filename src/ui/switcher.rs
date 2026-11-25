use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::{gdk, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::store::{Channel, slugify};
use crate::ui::media_util::{ThumbCache, format_relative};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SwitcherMode {
    Jump,
    MoveSelected,
}

pub enum SwitcherAction {
    OpenChannel(i64),
    MoveToChannel(i64),
    CreateChannel(String, bool),
}

pub fn present_switcher(
    parent: &impl IsA<gtk4::Widget>,
    root_dir: PathBuf,
    thumb_cache: Rc<RefCell<ThumbCache>>,
    channels: Vec<Channel>,
    mode: SwitcherMode,
    on_action: impl Fn(SwitcherAction) + 'static,
) {
    let dialog = adw::Dialog::builder()
        .content_width(460)
        .content_height(390)
        .build();

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("koma-switcher");

    let search_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    search_bar.set_margin_top(10);
    search_bar.set_margin_bottom(10);
    search_bar.set_margin_start(14);
    search_bar.set_margin_end(12);

    let entry = gtk4::SearchEntry::builder()
        .hexpand(true)
        .css_classes(["koma-switcher-entry"])
        .placeholder_text(match mode {
            SwitcherMode::Jump => "Jump to channel...",
            SwitcherMode::MoveSelected => "Move selected images to channel...",
        })
        .build();
    let esc_badge = gtk4::Label::builder()
        .label("ESC")
        .css_classes(["koma-kbd"])
        .build();
    search_bar.append(&entry);
    search_bar.append(&esc_badge);
    outer.append(&search_bar);

    let top_sep = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    top_sep.add_css_class("koma-switcher-sep");
    outer.append(&top_sep);

    let list = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["koma-sidebar-list"])
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    outer.append(&scroll);

    let bot_sep = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    bot_sep.add_css_class("koma-switcher-sep");
    let footer = gtk4::Label::builder()
        .label(match mode {
            SwitcherMode::Jump => "↵ Open channel   ·   ⇧↵ Move selected   ·   ↑↓ Navigate",
            SwitcherMode::MoveSelected => "↵ Move selected   ·   ↑↓ Navigate   ·   Esc Cancel",
        })
        .margin_top(8)
        .margin_bottom(9)
        .css_classes(["koma-tabular"])
        .build();
    outer.append(&bot_sep);
    outer.append(&footer);

    dialog.set_child(Some(&outer));

    #[derive(Clone)]
    enum RowKind {
        Channel(i64),
        Create(String),
    }

    let rows_state: Rc<RefCell<Vec<RowKind>>> = Rc::new(RefCell::new(Vec::new()));
    let on_action = Rc::new(on_action);

    let populate = Rc::new({
        let list = list.clone();
        let rows_state = rows_state.clone();
        let channels = channels.clone();
        let root_dir = root_dir.clone();
        let thumb_cache = thumb_cache.clone();
        move |query: &str| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut kinds = Vec::new();
            let q = query.trim().to_ascii_lowercase();
            let mut exact_match = false;

            for ch in &channels {
                if ch.slug == q {
                    exact_match = true;
                }
                if !q.is_empty() && !ch.slug.contains(&q) {
                    continue;
                }

                let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
                row_box.set_margin_top(6);
                row_box.set_margin_bottom(6);
                row_box.set_margin_start(8);
                row_box.set_margin_end(8);

                let thumb = gtk4::Picture::new();
                thumb.set_content_fit(gtk4::ContentFit::Cover);
                thumb.set_size_request(32, 32);
                thumb.set_overflow(gtk4::Overflow::Hidden);
                thumb.add_css_class("koma-switcher-thumb");
                if let Some(rel) = &ch.latest_thumb {
                    let tex = thumb_cache.borrow_mut().get_or_load(&root_dir.join(rel));
                    thumb.set_paintable(tex.as_ref());
                }
                row_box.append(&thumb);

                let text_col = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
                text_col.set_hexpand(true);
                let name = gtk4::Label::builder()
                    .label(format!("# {}", ch.slug))
                    .xalign(0.0)
                    .css_classes(["koma-inspector-filename"])
                    .build();
                let sub_text = match ch.last_added {
                    Some(ts) => format!("{} images · {}", ch.image_count, format_relative(ts)),
                    None => format!("{} images", ch.image_count),
                };
                let sub = gtk4::Label::builder()
                    .label(sub_text)
                    .xalign(0.0)
                    .css_classes(["koma-tabular"])
                    .build();
                text_col.append(&name);
                text_col.append(&sub);
                row_box.append(&text_col);

                let count = gtk4::Label::builder()
                    .label(ch.image_count.to_string())
                    .valign(gtk4::Align::Center)
                    .css_classes(["koma-tabular"])
                    .build();
                row_box.append(&count);

                list.append(&row_box);
                kinds.push(RowKind::Channel(ch.id));
            }

            if !q.is_empty()
                && !exact_match
                && let Ok(slug) = slugify(&q)
            {
                let create_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                create_row.set_margin_top(8);
                create_row.set_margin_bottom(8);
                create_row.set_margin_start(10);
                create_row.set_margin_end(10);
                let plus = gtk4::Label::builder()
                    .label(format!("+  Create channel #{slug}"))
                    .xalign(0.0)
                    .hexpand(true)
                    .build();
                create_row.append(&plus);
                list.append(&create_row);
                kinds.push(RowKind::Create(slug));
            }

            *rows_state.borrow_mut() = kinds;
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
        }
    });

    populate("");

    let pop_changed = populate.clone();
    entry.connect_search_changed(move |e| {
        pop_changed(&e.text());
    });

    let trigger_selected = Rc::new({
        let list = list.clone();
        let rows_state = rows_state.clone();
        let on_action = on_action.clone();
        let dialog = dialog.clone();
        move |shift_held: bool| {
            let Some(row) = list.selected_row() else {
                return;
            };
            let idx = row.index() as usize;
            let Some(kind) = rows_state.borrow().get(idx).cloned() else {
                return;
            };
            let move_mode = mode == SwitcherMode::MoveSelected || shift_held;
            dialog.close();
            match kind {
                RowKind::Channel(id) => {
                    if move_mode {
                        on_action(SwitcherAction::MoveToChannel(id));
                    } else {
                        on_action(SwitcherAction::OpenChannel(id));
                    }
                }
                RowKind::Create(slug) => {
                    on_action(SwitcherAction::CreateChannel(slug, move_mode));
                }
            }
        }
    });

    let trig_click = trigger_selected.clone();
    list.connect_row_activated(move |_, _| {
        trig_click(false);
    });

    let key_ctrl = gtk4::EventControllerKey::new();
    let list_nav = list.clone();
    let trig_key = trigger_selected.clone();
    let dlg_esc = dialog.clone();
    key_ctrl.connect_key_pressed(move |_, key, _, mods| match key {
        gdk::Key::Escape => {
            dlg_esc.close();
            glib::Propagation::Stop
        }
        gdk::Key::Down => {
            let next = list_nav
                .selected_row()
                .map(|r| r.index() + 1)
                .unwrap_or(0);
            if let Some(row) = list_nav.row_at_index(next) {
                list_nav.select_row(Some(&row));
            }
            glib::Propagation::Stop
        }
        gdk::Key::Up => {
            let prev = list_nav
                .selected_row()
                .map(|r| (r.index() - 1).max(0))
                .unwrap_or(0);
            if let Some(row) = list_nav.row_at_index(prev) {
                list_nav.select_row(Some(&row));
            }
            glib::Propagation::Stop
        }
        gdk::Key::Return | gdk::Key::KP_Enter => {
            trig_key(mods.contains(gdk::ModifierType::SHIFT_MASK));
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    entry.add_controller(key_ctrl);

    dialog.present(Some(parent));
    entry.grab_focus();
}
