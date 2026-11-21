//! Startup notification on Linux. A desktop that starts the app from its dock or menu
//! hands it a token (`XDG_ACTIVATION_TOKEN` on Wayland) and shows a spinning "starting"
//! cursor until a window of the app uses the token, or for about 15 seconds. eframe
//! does not pass the token on to the window, so [`activate`] does, through the window's
//! own Wayland connection (xdg-activation-v1).

use std::sync::OnceLock;

static TOKEN: OnceLock<Option<String>> = OnceLock::new();

/// Takes the token out of the environment, so terminals and programs started from the
/// app do not inherit it. Called first thing in `main`, before any thread starts.
pub fn take_token() {
    let token = std::env::var("XDG_ACTIVATION_TOKEN").ok().filter(|t| !t.is_empty());
    std::env::remove_var("XDG_ACTIVATION_TOKEN");
    std::env::remove_var("DESKTOP_STARTUP_ID");
    let _ = TOKEN.set(token);
}

/// Hands the token to the new window (Wayland only): the desktop stops its "starting"
/// cursor and gives the window the focus. A failure only leaves the cursor to time out.
pub fn activate(cc: &eframe::CreationContext) {
    #[cfg(target_os = "linux")]
    if let Some(token) = TOKEN.get().cloned().flatten() {
        if let Err(e) = wayland::activate(cc, &token) {
            eprintln!("matrix_finder: startup notification: {e}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cc;
}

#[cfg(target_os = "linux")]
mod wayland {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use wayland_backend::sys::client::{Backend, ObjectId};
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{self, XdgActivationV1};

    struct State;

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<XdgActivationV1, ()> for State {
        fn event(_: &mut Self, _: &XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    pub fn activate(cc: &eframe::CreationContext, token: &str) -> Result<(), String> {
        let display = cc.display_handle().map_err(|e| e.to_string())?.as_raw();
        let window = cc.window_handle().map_err(|e| e.to_string())?.as_raw();
        // On X11 the desktop matches the window by its class (StartupWMClass) instead.
        let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) = (display, window) else { return Ok(()) };
        // Safety: the display and the surface belong to the window eframe has just made,
        // which lives on after this call; the connection made here only borrows them.
        let backend = unsafe { Backend::from_foreign_display(d.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }.map_err(|e| e.to_string())?;
        let surface = WlSurface::from_id(&conn, id).map_err(|e| e.to_string())?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let activation: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).map_err(|e| e.to_string())?;
        activation.activate(token.to_string(), &surface);
        activation.destroy();
        queue.roundtrip(&mut State).map_err(|e| e.to_string())?;
        Ok(())
    }
}
