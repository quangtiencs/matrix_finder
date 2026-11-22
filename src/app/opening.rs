//! Opening things: the cursor's item, Open with, archive passwords, local copies of
//! files from servers and archives, and Quick View.
use super::*;

impl App {
    /// Enter / double-click on the cursor entry.
    pub(super) fn open_cursor(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let Some(e) = tab.cursor_entry().cloned() else { return };
        let loc = tab.loc.clone();
        if e.is_parent {
            self.panes[a].tab_mut().go_up(&lc!(self));
            return;
        }
        if e.is_dir {
            self.navigate_active(loc.child(&e.name));
            return;
        }
        match &loc {
            Location::Local(dir) => {
                let path = dir.join(&e.name);
                // An app chosen for archives takes over from browsing inside them.
                // Only a regular file is browsed as an archive (a named pipe would block).
                if archive::is_archive(&path) && path.is_file() && crate::system::defaultapps::program_for(&self.cfg.default_apps, &e.name).is_none() {
                    if archive::lock_state(&path) == archive::Lock::Everything {
                        self.ask_archive_password(path.clone(), AfterPassword::Browse(path));
                        return;
                    }
                    self.navigate_active(Location::Archive { file: path, inner: String::new() });
                } else {
                    self.open_file(&path);
                }
            }
            _ => self.open_via_temp(&e, false),
        }
    }

    /// Open a file with the app chosen for its type, or else the system's default app.
    pub(super) fn open_file(&mut self, path: &Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let opened = match crate::system::defaultapps::program_for(&self.cfg.default_apps, &name) {
            Some(program) => terminal::open_with(program, path).map_err(|e| format!("{} {program}: {e}", t("Could not open the file with"))),
            None => terminal::open_default(path),
        };
        if let Err(e) = opened {
            self.toast(e, true);
        }
    }

    /// The "Open with…" dialog, starting from the app chosen for this type.
    pub(super) fn open_with_dialog(&mut self, path: PathBuf) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let program = crate::system::defaultapps::program_for(&self.cfg.default_apps, &name).unwrap_or_default().to_string();
        self.show_dialog(Dialog::OpenWith { path, program, always: false }, true);
    }

    fn ask_archive_password(&mut self, file: PathBuf, then: AfterPassword) {
        self.show_dialog(Dialog::ArchivePassword { file, password: String::new(), wrong: false, checking: None, then }, true);
    }

    /// The password was accepted: go on with what it was asked for.
    pub(super) fn after_password(&mut self, then: AfterPassword) {
        self.quick = None;
        match then {
            AfterPassword::Browse(file) => self.navigate_active(Location::Archive { file, inner: String::new() }),
            AfterPassword::Preview => {}
            AfterPassword::Command(cmd) => self.run_cmd(cmd),
            AfterPassword::Confirm(dialog) => {
                if let Some(next) = self.confirm_dialog(*dialog) {
                    self.keep_dialog(next);
                }
            }
        }
    }

    /// Download / extract a non-local file to a temp folder and open it.
    pub(super) fn open_via_temp(&mut self, e: &Entry, with_dialog: bool) {
        let a = self.active;
        let loc = self.panes[a].tab().loc.clone();
        if let Location::Archive { file, inner } = &loc {
            if archive::entry_is_locked(file, &join_slash(inner, &e.name)) {
                let cmd = if with_dialog { Cmd::OpenWith } else { Cmd::Open };
                self.ask_archive_password(file.clone(), AfterPassword::Command(cmd));
                return;
            }
        }
        let Some(src) = self.endpoint(&loc) else { return };
        // A new private folder each time: an earlier copy (maybe edited) is never written over.
        let tmp = match crate::util::private_temp_dir(crate::files::tempcopy::TEMP_PREFIX) {
            Ok(d) => d,
            Err(err) => {
                self.toast(format!("{} {}: {err}", t("Could not open"), e.name), true);
                return;
            }
        };
        let target = tmp.join(&e.name);
        let label = loc.child(&e.name).display(|id| self.remote_label(id));
        self.temp_copies.add(loc.clone(), e.name.clone(), label, tmp.clone(), target.clone());
        let items = vec![ops::Item { name: e.name.clone(), is_dir: false, size: e.size }];
        let dst = Endpoint::Local(tmp);
        let after = if with_dialog { After::OpenWithDialog(target) } else { After::OpenFile(target) };
        let sources = local_file_of(&loc).into_iter().collect();
        self.spawn_job(format!("{} {}", t("Opening"), e.name), after, None, sources, move |p| {
            ops::transfer(src, items, dst, false, Conflict::Skip, p)
        });
    }

    /// Every two seconds: a copy opened from a server or an archive that an app saved is
    /// asked about (upload the changes, or keep the copy).
    pub(super) fn poll_temp_copies(&mut self, ctx: &egui::Context) {
        if self.temp_copies.list.is_empty() {
            return;
        }
        for id in self.temp_copies.poll(false) {
            if !self.copy_prompts.contains(&id) {
                self.copy_prompts.push(id);
            }
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    /// Upload the edited copy `id` over the file it came from on the server.
    fn upload_copy(&mut self, id: u64) {
        let Some(c) = self.temp_copies.list.iter().find(|c| c.id == id) else { return };
        let (src, name, dir, label) = (c.src.clone(), c.name.clone(), c.dir.clone(), c.label.clone());
        let Some(stamp) = crate::files::tempcopy::stamp(&c.path) else { return };
        let Some(dst) = self.endpoint(&src) else {
            self.toast(format!("{}: {label}", t("Not connected any more; the edited copy is kept")), true);
            return;
        };
        if let Some(c) = self.temp_copies.get_mut(id) {
            c.uploading = true;
            c.asked = Some(stamp);
        }
        let items = vec![ops::Item { name: name.clone(), is_dir: false, size: stamp.0 }];
        self.spawn_job(format!("{} {name}", t("Uploading changes to")), After::Uploaded(id, stamp), Some(src), vec![dir.clone()], move |p| {
            ops::transfer(Endpoint::Local(dir), items, dst, false, Conflict::Overwrite, p)
        });
    }

    /// The notes about edited copies, above the toasts.
    pub(super) fn copy_prompts_ui(&mut self, ctx: &egui::Context) {
        self.copy_prompts.retain(|id| self.temp_copies.list.iter().any(|c| c.id == *id && !c.uploading));
        if self.copy_prompts.is_empty() {
            return;
        }
        let th = self.theme;
        let mut act: Option<(u64, u8)> = None;
        egui::Area::new(egui::Id::new("copy-prompts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 64.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for c in self.temp_copies.list.iter().filter(|c| self.copy_prompts.contains(&c.id)) {
                    egui::Frame::popup(ui.style()).fill(th.panel).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 10)).stroke(egui::Stroke::new(1.0_f32, th.border)).show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(RichText::new(format!("{} “{}”", t("Changed in another app:"), c.name)).strong().color(th.text));
                        ui.add(egui::Label::new(RichText::new(&c.label).color(th.weak)).truncate());
                        if !c.is_remote() {
                            ui.label(RichText::new(t("It came from an archive, which cannot be changed: keep the copy somewhere.")).color(th.weak));
                        }
                        ui.horizontal(|ui| {
                            if c.is_remote() {
                                let b = egui::Button::new(RichText::new(t("Upload changes")).color(th.on_accent()).strong()).fill(th.accent);
                                if ui.add(b).clicked() {
                                    act = Some((c.id, 0));
                                }
                            } else if ui.button(t("Show copy")).clicked() {
                                act = Some((c.id, 1));
                            }
                            if ui.button(t("Keep local copy")).clicked() {
                                act = Some((c.id, 2));
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        let Some((id, what)) = act else { return };
        match what {
            0 => self.upload_copy(id),
            1 => {
                if let Some(c) = self.temp_copies.list.iter().find(|c| c.id == id) {
                    if let Err(e) = terminal::reveal(&c.path) {
                        self.toast(e, true);
                    }
                }
            }
            _ => {
                if let Some(c) = self.temp_copies.get_mut(id) {
                    c.asked = crate::files::tempcopy::stamp(&c.path);
                }
                let dir = crate::files::tempcopy::recovered_dir();
                self.toast(format!("{} {}", t("Kept. When Matrix Finder quits, edited copies are moved to"), dir.display()), false);
            }
        }
        if what != 1 {
            self.copy_prompts.retain(|x| *x != id);
        }
    }

    pub(super) fn preview_for_cursor(&self, side: usize) -> Option<Preview> {
        let tab = self.panes[side].tab();
        let e = tab.cursor_entry()?.clone();
        let key = format!("{}|{}", tab.loc.display(|id| self.remote_label(id)), e.name);
        let syntax = self.theme.syntax;
        if e.is_parent {
            return Some(Preview::info(key, "..".into(), t("Parent folder")));
        }
        let limit = preview::read_limit(&e.name);
        match &tab.loc {
            Location::Local(dir) => {
                let path = dir.join(&e.name);
                if e.is_dir {
                    let name = e.name.clone();
                    return Some(Preview::text_async(key, e.name.clone(), move |cancel| folder_summary(&path, &name, cancel)));
                }
                // Only asks the file system (about the target, for a link). Only regular
                // files are read: a named pipe or a device could block forever, or give
                // away data meant for another program.
                let size = match std::fs::metadata(&path) {
                    Ok(m) if m.is_file() => m.len(),
                    Ok(_) => return Some(Preview::info(key, e.name.clone(), special_file_note(&path))),
                    Err(err) => return Some(Preview::info(key, e.name.clone(), err.to_string())),
                };
                if preview::too_large(&e.name, size, None) {
                    return Some(Preview::too_large(key, e.name.clone(), size));
                }
                // Comic books are archives shown by their cover.
                if archive::is_archive(&path) && preview::classify(&e.name) != preview::Kind::Package {
                    return Some(Preview::text_async(key, e.name.clone(), move |cancel| archive_summary(&path, cancel)));
                }
                if preview::classify(&e.name) == preview::Kind::Table {
                    return Some(Preview::table_local(key, e.name.clone(), path, syntax));
                }
                if Preview::reads_files_directly(&e.name) {
                    return Some(Preview::local_file(key, e.name.clone(), path, syntax, self.max_texture));
                }
                let loader: preview::Loader = Box::new(move || read_head(&path, limit));
                Some(Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).of_size(size))
            }
            Location::Archive { file, inner } => {
                if e.is_dir {
                    return Some(Preview::info(key, e.name.clone(), format!("{}: {}", t("Folder in archive"), human_size(e.size))));
                }
                if preview::too_large(&e.name, e.size, Some(limit as u64)) {
                    return Some(Preview::too_large(key, e.name.clone(), e.size));
                }
                let file = file.clone();
                let inner = join_slash(inner, &e.name);
                let archive_file = file.clone();
                let loader: preview::Loader = Box::new(move || archive::read_entry(&file, &inner, limit));
                Some(Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).in_archive(archive_file).of_size(e.size))
            }
            Location::Remote { id, path } => {
                if e.is_dir {
                    return Some(Preview::info(key, e.name.clone(), t("Remote folder")));
                }
                let fs = self.remotes.get(id)?.1.clone();
                let rp = join_slash(path, &e.name);
                let limit = limit.min(16 * 1024 * 1024);
                if preview::too_large(&e.name, e.size, Some(limit as u64)) {
                    return Some(Preview::too_large(key, e.name.clone(), e.size));
                }
                // The preview's cancel flag, once it exists: moving on stops the read. A
                // transfer holds the connection for a whole file, so this does not wait for it.
                let flag: Arc<std::sync::OnceLock<crate::preview::media::Cancel>> = Arc::default();
                let f2 = flag.clone();
                let loader: preview::Loader = Box::new(move || {
                    // It is set right after the preview is made.
                    let cancel = (0..1000).find_map(|_| f2.get().cloned().or_else(|| { std::thread::sleep(Duration::from_millis(1)); None })).unwrap_or_default();
                    let gone = || cancel.load(std::sync::atomic::Ordering::Relaxed);
                    let mut g = remote::use_soon(&fs, Duration::from_secs(2), &gone)?;
                    g.read_head(&rp, limit, &cancel)
                });
                let p = Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).of_size(e.size);
                let _ = flag.set(p.cancel_flag());
                Some(p)
            }
        }
    }

    /// The single Quick View panel (right side), showing the active pane's item.
    pub(super) fn quick_view_panel(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let a = self.active;
        let want_key = {
            let tab = self.panes[a].tab();
            tab.cursor_entry().map(|e| format!("{}|{}", tab.loc.display(|id| self.remote_label(id)), e.name))
        };
        if want_key.is_some() && self.quick.as_ref().map(|q| Some(&q.key)) != Some(want_key.as_ref()) {
            self.quick = self.preview_for_cursor(a);
        } else if want_key.is_none() && self.panes[a].tab().is_files() {
            // An empty list (a filter that matches nothing) has nothing to preview.
            self.quick = None;
        }
        egui::Frame::NONE
            .fill(th.bg)
            .corner_radius((th.radius * 2).max(4))
            .stroke(egui::Stroke::new(1.0_f32, th.border))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                // Fixed size: previews never resize the panel.
                let size = ui.available_size();
                ui.set_min_size(size);
                ui.set_max_size(size);
                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                match &mut self.quick {
                    Some(q) => q.ui(ui, th, t),
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new(t("Nothing to preview")).color(th.weak));
                        });
                    }
                }
            });
        if let Some(file) = self.quick.as_mut().and_then(|q| q.take_password_request()) {
            self.ask_archive_password(file, AfterPassword::Preview);
        }
    }

    pub(super) fn toggle_quick_view(&mut self) {
        self.cfg.quick_view = !self.cfg.quick_view;
        self.quick = None;
    }
}

/// What Quick View says about a named pipe, socket or device: it is not opened.
fn special_file_note(path: &Path) -> String {
    let kind = match crate::util::special_kind(path) {
        Some("Named pipe") => t("Named pipe"),
        Some("Socket") => t("Socket"),
        Some("Device") => t("Device"),
        _ => t("Special file"),
    };
    format!("{kind}\n\n{}", t("Not opened: only regular files are previewed, so nothing waits on it or takes data meant for another program."))
}

fn read_head(path: &Path, limit: usize) -> Res<Vec<u8>> {
    use std::io::Read;
    let f = crate::util::open_regular(path).map_err(es)?;
    let mut buf = Vec::new();
    f.take(limit as u64).read_to_end(&mut buf).map_err(es)?;
    Ok(buf)
}

/// How long a folder or archive summary may read before it shows what it has.
const SUMMARY_TIME: Duration = Duration::from_secs(4);

/// Most names a summary collects (and sorts) to show its first few.
const SUMMARY_NAMES: usize = 10_000;

/// Names shown in a summary.
const SUMMARY_SHOWN: usize = 40;

/// The last line of a summary list: how many names were left out.
fn summary_rest(left: usize, cut: bool) -> String {
    match (left, cut) {
        (_, true) => format!("\n{}", t("…and more")),
        (0, false) => String::new(),
        (n, false) => format!("\n… {n} {}", t("more not shown")),
    }
}

fn folder_summary(path: &Path, name: &str, cancel: &crate::preview::media::Cancel) -> String {
    let started = Instant::now();
    let mut files = 0;
    let mut dirs = 0;
    let mut size = 0u64;
    let mut names: Vec<(bool, String)> = vec![];
    let mut cut = false;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return String::new();
            }
            if started.elapsed() > SUMMARY_TIME {
                cut = true;
                break;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs += 1;
            } else {
                files += 1;
                size += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
            // Past this many the counts go on, but only these names are sorted.
            if names.len() < SUMMARY_NAMES {
                names.push((is_dir, e.file_name().to_string_lossy().to_string()));
            }
        }
    }
    // Folders first, then by name.
    names.sort_by_cached_key(|(is_dir, n)| (!is_dir, n.to_lowercase()));
    let shown: Vec<String> = names.iter().take(SUMMARY_SHOWN).map(|(is_dir, n)| format!("{} {n}", if *is_dir { "📁" } else { "🗋" })).collect();
    let left = (dirs + files) - shown.len();
    let more = if cut { "+" } else { "" };
    format!(
        "📁 {name}\n\n{dirs}{more} {}, {files}{more} {}\n{}{more} {}\n\n{}{}",
        t("folders"),
        t("files"),
        human_size(size),
        t("in the files at this level (not counting subfolders)"),
        shown.join("\n"),
        summary_rest(left, cut)
    )
}

fn archive_summary(path: &Path, cancel: &crate::preview::media::Cancel) -> String {
    let started = Instant::now();
    let stop = || cancel.load(std::sync::atomic::Ordering::Relaxed) || started.elapsed() > SUMMARY_TIME;
    match archive::list_until(path, 200_000, &stop) {
        _ if cancel.load(std::sync::atomic::Ordering::Relaxed) => String::new(),
        Err(e) if archive::is_password_error(&e) => format!("🔒 {}\n\n{}", t("Password protected"), t("Press Enter to enter the password.")),
        Ok((entries, cut)) => {
            let files = entries.iter().filter(|e| !e.is_dir).count();
            let total: u64 = entries.iter().map(|e| e.size).sum();
            let packed = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let ratio = if total > 0 && !cut { format!(" ({:.0}%)", packed as f64 * 100.0 / total as f64) } else { String::new() };
            let more = if cut { "+" } else { "" };
            let top = archive::children(&entries, "");
            let list: Vec<String> = top
                .iter()
                .take(60)
                .map(|e| format!("{} {}  {}", if e.is_dir { "📁" } else { "🗋" }, e.name, if e.is_dir { String::new() } else { human_size(e.size) }))
                .collect();
            format!(
                "📦 {}\n\n{files}{more} {} · {}{more} → {}{ratio}\n{}\n\n{}{}",
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                t("files"),
                human_size(total),
                human_size(packed),
                if archive::lock_state(path) == archive::Lock::Open { t("Press Enter to browse inside the archive.") } else { format!("🔒 {}. {}", t("Password protected"), t("Press Enter to browse inside the archive.")) },
                list.join("\n"),
                summary_rest(top.len() - list.len(), cut)
            )
        }
        Err(e) => format!("{}: {e}", t("Cannot read archive")),
    }
}
