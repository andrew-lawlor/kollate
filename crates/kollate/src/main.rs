//! Kollate: curate Kobo highlights, notes and vocabulary.

mod card;
mod edit;
mod shortcuts;
mod window;
mod word;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use kollate_core::{Library, default_library_path};

/// Development builds use a separate ID, so they run alongside an installed
/// Kollate instead of handing their launch over to it.
pub const APP_ID: &str = if cfg!(debug_assertions) {
    "io.github.andrew_lawlor.Kollate.Devel"
} else {
    "io.github.andrew_lawlor.Kollate"
};

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(activate);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("win.import", &["<Control>i"]);
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    app.set_accels_for_action("win.export", &["<Control>e"]);
    app.set_accels_for_action("win.back", &["<Alt>Left"]);
    app.set_accels_for_action("win.shortcuts", &["<Control>question", "<Control>slash"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.run()
}

/// The library database; `KOLLATE_LIBRARY` overrides it (for development).
fn library_path() -> std::path::PathBuf {
    std::env::var_os("KOLLATE_LIBRARY")
        .map(Into::into)
        .unwrap_or_else(default_library_path)
}

fn load_css() {
    gtk::Window::set_default_icon_name(APP_ID);
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("a display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn activate(app: &adw::Application) {
    if let Some(win) = app.active_window() {
        win.present();
        return;
    }

    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| app.quit()
    ));
    app.add_action(&quit);

    let path = library_path();
    match Library::open(&path) {
        Ok(library) => window::Window::new(app, library).present(),
        Err(err) => {
            let win = adw::ApplicationWindow::builder().application(app).build();
            win.present();
            let dialog = adw::AlertDialog::new(
                Some("Couldn’t Open Library"),
                Some(&format!("{}\n\n{err}", path.display())),
            );
            dialog.add_response("close", "Close");
            dialog.connect_response(None, move |_, _| win.close());
            dialog.present(None::<&gtk::Widget>);
        }
    }
}
