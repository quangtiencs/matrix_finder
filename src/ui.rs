//! The file manager's own interface: windows and tabs, their layout, the command palette,
//! folder fields, the quick filter, the title bar and the keyboard shortcuts.
pub mod chrome;
pub mod completion;
pub mod filter;
pub mod foldertree;
pub mod layout;
pub mod palette;
pub mod pane;
pub mod pathfield;
pub mod search;
pub mod shortcuts;
pub mod tabs;

/// A popup below `widget`, at least as wide as it, open while `id` is (`egui::Popup::open_id`,
/// `toggle_id`); `None` when it is closed. What egui's old `popup_below_widget` did.
pub fn popup_below<R>(id: egui::Id, widget: &egui::Response, close: egui::PopupCloseBehavior, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    egui::Popup::from_response(widget)
        .layout(egui::Layout::top_down_justified(egui::Align::LEFT))
        .open_memory(None)
        .close_behavior(close)
        .id(id)
        .align(egui::RectAlign::BOTTOM_START)
        .width(widget.rect.width())
        .show(|ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .map(|r| r.inner)
}

/// Menus that close on a click outside them, or when an item closes them
/// (`ui.close_kind(UiKind::Menu)`), as egui's did before 0.32: ticking a box, typing in a
/// field or trying themes one after another keeps the menu open.
pub fn kept_menu_config() -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
}

/// A right-click menu that stays open like the ones of `kept_menu_config`.
pub trait KeptContextMenu {
    fn kept_context_menu(&self, add: impl FnOnce(&mut egui::Ui)) -> Option<egui::InnerResponse<()>>;
}

impl KeptContextMenu for egui::Response {
    fn kept_context_menu(&self, add: impl FnOnce(&mut egui::Ui)) -> Option<egui::InnerResponse<()>> {
        egui::Popup::context_menu(self).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(add)
    }
}
