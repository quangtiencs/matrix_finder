//! A text field that takes a folder: the folders matching what is typed show under it
//! (↑ ↓ choose, Tab completes, Enter or a click fills one in, Esc closes the list), and
//! a button beside it opens the folder tree. Used wherever a folder is typed: Copy /
//! Move, Compress, Extract, Backup and the bookmarks.
//!
//! One [`PathField`] serves several fields: the one being typed in (or whose tree is
//! open) owns the suggestions and the tree, as only one field has the keyboard at a time.

use crate::completion::Completer;
use crate::foldertree::{self, FolderTree, Popup};
use crate::i18n::t;
use crate::theme::Theme;
use egui::{Id, Key, Modifiers};
use std::path::{Path, PathBuf};

/// The folder a typed path names: `~` is the home folder, and a relative path is taken
/// from `base`. None for an empty or relative path with no base.
pub fn typed_dir(text: &str, home: Option<&Path>, base: Option<&Path>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let p = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => home?.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(text),
    };
    if p.is_absolute() {
        Some(p)
    } else {
        Some(base?.join(p))
    }
}

/// An address as typed, made whole: `~` is the home folder and a relative path is taken
/// from `base` (for an archive, its file: `b.zip::docs`). Server addresses, and relative
/// paths with no base, stay as typed.
pub fn resolve_typed(text: &str, home: Option<&Path>, base: Option<&Path>) -> String {
    let text = text.trim();
    if text.contains("://") {
        return text.to_string();
    }
    if let Some((file, inner)) = text.split_once("::") {
        return match typed_dir(file, home, base) {
            Some(p) => format!("{}::{inner}", p.display()),
            None => text.to_string(),
        };
    }
    typed_dir(text, home, base).map(|p| p.display().to_string()).unwrap_or_else(|| text.to_string())
}

/// Put the text cursor of field `id` at the end of `text`.
pub fn cursor_to_end(ctx: &egui::Context, id: Id, text: &str) {
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    st.store(ctx, id);
}

/// How a field is shown.
pub struct Opts<'a> {
    /// Where a relative path starts.
    pub base: Option<&'a Path>,
    pub show_hidden: bool,
    /// The drives, for the tree.
    pub drives: &'a [crate::drives::Drive],
    /// The width of the field and its button together (at most the width left).
    pub width: f32,
    pub hint: Option<String>,
}

#[derive(Default)]
pub struct PathField {
    /// The field the suggestions and the tree belong to.
    owner: Option<Id>,
    pub complete: Completer,
    pub tree: Option<FolderTree>,
    /// Give the field the keyboard (back) in the next frame.
    focus: bool,
    /// The frame whose keys were taken.
    keys_pass: Option<u64>,
}

impl PathField {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The tree, while one of field `id` is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tree_of(&self, id: Id) -> Option<&FolderTree> {
        self.tree.as_ref().filter(|_| self.owner == Some(id))
    }

    /// Keys and clicks for field `id`'s suggestions and tree, taken before anything else
    /// sees them (a dialog would take Enter and Esc for itself). [`Self::show`] calls it
    /// too; it acts once a frame.
    pub fn keys(&mut self, ctx: &egui::Context, text: &mut String, id: Id) {
        if self.owner != Some(id) || self.keys_pass == Some(ctx.cumulative_pass_nr()) {
            return;
        }
        self.keys_pass = Some(ctx.cumulative_pass_nr());
        if let Some(tree) = self.tree.as_mut() {
            match tree.keys(ctx) {
                Some(Popup::Picked(p)) => self.take(ctx, text, id, &p),
                Some(Popup::Closed) => {
                    self.tree = None;
                    self.focus = true;
                }
                _ => {}
            }
            return;
        }
        let comp = &mut self.complete;
        let mut filled = false;
        // A click on a suggestion. The rows are the ones drawn in the last frame: the
        // field gives up the keyboard as soon as the mouse is pressed elsewhere.
        let press = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
        if let Some(s) = press.and_then(|p| comp.rows.iter().position(|r| r.contains(p))).and_then(|k| comp.items.get(k)) {
            *text = s.fill.clone();
            filled = true;
            self.focus = true;
        } else if !comp.rows.is_empty() {
            // The list is shown (it only is while the field has the keyboard). egui lets go
            // of the field on Esc before this runs, so it is given the keyboard back.
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                comp.reset();
                self.focus = true;
                return;
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                comp.step(1);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                comp.step(-1);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
                if let Some(t) = comp.tab(text) {
                    *text = t;
                    filled = true;
                }
            }
            if let Some(s) = comp.sel.and_then(|k| comp.items.get(k)) {
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                    *text = s.fill.clone();
                    filled = true;
                }
            }
        }
        if filled {
            // Go on with the folders inside it.
            comp.active = true;
            cursor_to_end(ctx, id, text);
        }
    }

    /// A folder taken from the tree.
    fn take(&mut self, ctx: &egui::Context, text: &mut String, id: Id, p: &Path) {
        *text = p.display().to_string();
        self.tree = None;
        self.complete.reset();
        self.focus = true;
        cursor_to_end(ctx, id, text);
    }

    /// Draws field `id` and its tree button, and the suggestions or the tree when they
    /// are open. Returns the text field's response, changed also when a suggestion or
    /// the tree filled it in.
    pub fn show(&mut self, ui: &mut egui::Ui, text: &mut String, id: Id, o: Opts, th: &Theme) -> egui::Response {
        let before = text.clone();
        let mut r = self.field(ui, text, id, o, th);
        if *text != before {
            r.mark_changed();
        }
        r
    }

    fn field(&mut self, ui: &mut egui::Ui, text: &mut String, id: Id, o: Opts, th: &Theme) -> egui::Response {
        let ctx = ui.ctx().clone();
        self.keys(&ctx, text, id);
        let mine = self.owner == Some(id);
        let tree_open = mine && self.tree.is_some();
        let (r, btn) = ui
            .horizontal(|ui| {
                // Exactly the width left for it (the width given is inside its margins), or
                // a dialog would grow a little every frame.
                let margin = egui::Margin::symmetric(4, 2);
                let w = o.width.min(ui.available_width()) - 28.0 - ui.spacing().item_spacing.x - margin.sum().x;
                // lock_focus: Tab completes instead of moving to the next widget.
                let mut edit = egui::TextEdit::singleline(text).id(id).margin(margin).desired_width(w).lock_focus(true);
                if let Some(h) = &o.hint {
                    edit = edit.hint_text(egui::RichText::new(h).color(th.weak));
                }
                let r = ui.add(edit);
                let accent = th.accent;
                let b = crate::icons::icon_button(ui, 28.0, true, th, |p, r, c| foldertree::icon(p, r, if tree_open { accent } else { c })).on_hover_text(t("Folder tree"));
                b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Folder tree")));
                (r, b)
            })
            .inner;
        if (r.has_focus() || r.changed()) && !mine {
            // Typing in another field: the suggestions are this one's now.
            self.reset();
            self.owner = Some(id);
        }
        if self.owner != Some(id) {
            if btn.clicked() {
                self.reset();
                self.owner = Some(id);
                self.open_tree(text, &o);
            }
            return r;
        }
        if std::mem::take(&mut self.focus) {
            r.request_focus();
        }
        // Folders that match what is typed, under the field.
        let home = dirs::home_dir();
        let comp = &mut self.complete;
        comp.folders_only = true;
        comp.active |= r.changed();
        comp.update(text, home.as_deref(), o.base, o.show_hidden, &ctx);
        comp.rows.clear();
        if r.has_focus() && !comp.items.is_empty() && !tree_open {
            crate::pane::suggestion_popup(ui, id.with("suggestions"), r.rect, comp, th);
        }
        if btn.clicked() {
            if tree_open {
                self.tree = None;
            } else {
                self.open_tree(text, &o);
            }
        }
        if let Some(tree) = self.tree.as_mut() {
            match foldertree::popup(ui, id.with("tree"), btn.rect, tree, th) {
                Popup::Picked(p) => self.take(&ctx, text, id, &p),
                Popup::Closed => self.tree = None,
                Popup::Open => {}
            }
        }
        r
    }

    /// The tree, open at the folder typed (when there is one).
    fn open_tree(&mut self, text: &str, o: &Opts) {
        let home = dirs::home_dir();
        let current = typed_dir(text, home.as_deref(), o.base).filter(|p| p.is_dir());
        let roots = foldertree::roots(home.as_deref(), o.drives);
        self.tree = Some(FolderTree::new(roots, current.as_deref(), o.show_hidden));
        self.complete.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_paths() {
        let home = Some(Path::new("/home/me"));
        let base = Some(Path::new("/work"));
        assert_eq!(typed_dir("/tmp/x", home, base), Some(PathBuf::from("/tmp/x")));
        assert_eq!(typed_dir("  ~/Documents ", home, base), Some(PathBuf::from("/home/me/Documents")));
        assert_eq!(typed_dir("~", home, base), Some(PathBuf::from("/home/me")));
        assert_eq!(typed_dir("out/a", home, base), Some(PathBuf::from("/work/out/a")));
        // "~name" is a folder called that, not someone's home.
        assert_eq!(typed_dir("~old", home, base), Some(PathBuf::from("/work/~old")));
        assert_eq!(typed_dir("out", home, None), None);
        assert_eq!(typed_dir("~/x", None, base), None);
        assert_eq!(typed_dir("   ", home, base), None);
    }

    #[test]
    fn typed_addresses() {
        let home = Some(Path::new("/home/me"));
        let base = Some(Path::new("/work"));
        assert_eq!(resolve_typed("~/Documents/", home, base), "/home/me/Documents/");
        assert_eq!(resolve_typed("out/a", home, base), "/work/out/a");
        assert_eq!(resolve_typed(" /tmp ", home, base), "/tmp");
        // An archive: its file is resolved, the path inside it kept.
        assert_eq!(resolve_typed("b.zip::docs/x", home, base), "/work/b.zip::docs/x");
        // Servers, and relative paths with nowhere to start from, as typed.
        assert_eq!(resolve_typed("sftp://me@host/srv", home, base), "sftp://me@host/srv");
        assert_eq!(resolve_typed("out", home, None), "out");
        assert_eq!(resolve_typed("", home, base), "");
    }
}
