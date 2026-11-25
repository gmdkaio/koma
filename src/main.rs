use gtk4::{gdk, gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

const APP_ID: &str = "dev.koma.App";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let sidebar_list = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();

    let empty_row = gtk4::Label::builder()
        .label("No channels yet")
        .xalign(0.0)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .css_classes(["dim-label"])
        .build();
    sidebar_list.append(&empty_row);

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
        .title("koma")
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
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, mods| {
            if mods.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(key, gdk::Key::v | gdk::Key::V)
            {
                let clipboard = status.display().clipboard();
                clipboard.read_texture_async(
                    gio::Cancellable::NONE,
                    glib::clone!(
                        #[weak]
                        status,
                        move |res| match res {
                            Ok(Some(tex)) => status.set_description(Some(&format!(
                                "Clipboard image pasted: {}x{}",
                                tex.width(),
                                tex.height()
                            ))),
                            Ok(None) => status
                                .set_description(Some("Clipboard has no image texture.")),
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
        #[upgrade_or]
        false,
        move |_, value, _, _| {
            if let Ok(file_list) = value.get::<gdk::FileList>() {
                let paths: Vec<String> = file_list
                    .files()
                    .into_iter()
                    .filter_map(|f| f.path().map(|p| p.display().to_string()))
                    .collect();
                if !paths.is_empty() {
                    status.set_description(Some(&format!("Dropped:\n{}", paths.join("\n"))));
                    return true;
                }
            }
            false
        }
    ));
    window.add_controller(drop_target);

    window.present();
}
