//! Main-thread tray lifetime. The application supplies policy and handles events.
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Settings,
    Quit,
}

fn action(id: &str) -> Option<Action> {
    match id {
        "newport-tray-open" => Some(Action::Open),
        "newport-tray-settings" => Some(Action::Settings),
        "newport-tray-quit" => Some(Action::Quit),
        _ => None,
    }
}

/// Keep this owner on the GUI event-loop thread; dropping it removes the icon.
pub struct Tray(TrayIcon);
impl Tray {
    pub fn new(on_action: impl Fn(Action) + Send + Sync + 'static) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        if objc2::MainThreadMarker::new().is_none() {
            return Err("Tray installation requires the main thread".into());
        }
        let open = MenuItem::with_id("newport-tray-open", "Open Newport", true, None);
        let settings = MenuItem::with_id("newport-tray-settings", "Settings…", true, None);
        let quit = MenuItem::with_id("newport-tray-quit", "Quit Newport", true, None);
        let menu = Menu::with_items(&[&open, &settings, &quit]).map_err(|e| e.to_string())?;
        let mut decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!(
            "../../../resources/icons/tray.png"
        )));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut bytes = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or("Invalid tray icon size")?
        ];
        let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
        if info.color_type != png::ColorType::Rgba {
            return Err("Tray icon must be RGBA".into());
        }
        bytes.truncate(info.buffer_size());
        let icon = tray_icon::Icon::from_rgba(bytes, info.width, info.height)
            .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_id("newport")
            .with_tooltip("Newport · SSH tunnels")
            .with_icon(icon)
            .with_icon_as_template(true)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| e.to_string())?;
        // Muda installs this callback once for the process. The application owns
        // it; after shutdown its closed dispatch channel rejects further work.
        // Unknown menu IDs are ignored.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = action(event.id.as_ref()) {
                on_action(action);
            }
        }));
        Ok(Self(tray))
    }
    pub fn set_status(&self, status: Option<(usize, bool)>) -> Result<(), String> {
        let Some((connected, errors)) = status else {
            self.0.set_title(Some("!"));
            return self
                .0
                .set_tooltip(Some("Newport · Tunnel status unavailable"))
                .map_err(|e| e.to_string());
        };
        let title = if connected > 0 {
            connected.to_string()
        } else if errors {
            "!".into()
        } else {
            String::new()
        };
        self.0.set_title(Some(title));
        self.0
            .set_tooltip(Some(format!("Newport · {connected} connected tunnels")))
            .map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tray_routes_only_owned_menu_items() {
        assert_eq!(action("newport-tray-open"), Some(Action::Open));
        assert_eq!(action("newport-tray-settings"), Some(Action::Settings));
        assert_eq!(action("newport-tray-quit"), Some(Action::Quit));
        assert_eq!(action("quit"), None);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn worker_thread_cannot_install_tray() {
        assert!(
            std::thread::spawn(|| Tray::new(|_| {}).err().unwrap())
                .join()
                .unwrap()
                .contains("main thread")
        );
    }
}
