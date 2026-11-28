//! Background jobs (copy, move, delete, extract, …): starting and polling them, the
//! Transfers window, and files dropped onto folders or from other apps.
use super::*;

impl App {
    /// Start a background job. File operations (`After::Refresh`) go through the
    /// transfer queue; quick "open this file" downloads start immediately.
    /// `sources` are the local paths it reads; the destination is added to them.
    pub(super) fn spawn_job<F>(&mut self, title: String, after: After, dest: Option<Location>, sources: Vec<PathBuf>, f: F)
    where
        F: FnOnce(&ops::Progress) -> Res<String> + Send + 'static,
    {
        let queued = matches!(after, After::Refresh);
        let job = Job::spawn(title, queued, f);
        let now = Instant::now();
        let mut paths = sources;
        paths.extend(dest.as_ref().and_then(local_file_of));
        self.jobs.push(RunningJob { job, after, dest, paths, speed: 0.0, sample: (now, 0), started_running: (!queued).then_some(now) });
        self.schedule_jobs();
    }

    /// Release queued jobs while fewer than `max_parallel` are running.
    fn schedule_jobs(&mut self) {
        let max = self.cfg.max_parallel.max(1);
        let mut running = self.jobs.iter().filter(|j| !j.job.progress.is_queued()).count();
        for j in &mut self.jobs {
            if running >= max {
                break;
            }
            if j.job.progress.is_queued() {
                j.job.progress.release();
                j.started_running = Some(Instant::now());
                running += 1;
            }
        }
    }

    pub(super) fn poll_jobs(&mut self, ctx: &egui::Context) {
        // Speed sampling (exponential moving average over ~0.5 s windows).
        let now = Instant::now();
        for j in &mut self.jobs {
            let dt = now.duration_since(j.sample.0).as_secs_f64();
            if dt >= 0.5 {
                let done = j.job.progress.done();
                let inst = done.saturating_sub(j.sample.1) as f64 / dt;
                j.speed = if j.speed == 0.0 { inst } else { j.speed * 0.6 + inst * 0.4 };
                if j.job.progress.is_paused() || j.job.progress.is_queued() {
                    j.speed = 0.0;
                }
                j.sample = (now, done);
            }
        }
        let mut finished = vec![];
        for (i, j) in self.jobs.iter().enumerate() {
            if let Some(r) = j.job.poll() {
                finished.push((i, r));
            }
        }
        let mut refresh = false;
        for (i, r) in finished.into_iter().rev() {
            let rj = self.jobs.remove(i);
            let secs = rj.started_running.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0);
            let cancelled = rj.job.progress.cancelled();
            let entry = |ok: bool, msg: String| FinishedJob {
                title: rj.job.title.clone(),
                ok,
                cancelled: !ok && cancelled,
                msg,
                bytes: rj.job.progress.done(),
                items: rj.job.progress.counts_items(),
                secs,
                dest: rj.dest.clone(),
                at: chrono::Local::now().format("%H:%M:%S").to_string(),
            };
            match r {
                Ok(msg) => match rj.after {
                    After::OpenFile(p) => {
                        self.temp_copies.downloaded(&p);
                        self.open_file(&p)
                    }
                    After::OpenWithDialog(p) => {
                        self.temp_copies.downloaded(&p);
                        self.open_with_dialog(p)
                    }
                    After::Refresh | After::Uploaded(..) => {
                        if let After::Uploaded(id, stamp) = rj.after {
                            if let Some(c) = self.temp_copies.get_mut(id) {
                                c.uploading = false;
                                c.saved = Some(stamp);
                            }
                        }
                        refresh = true;
                        self.toast(format!("✔ {}: {msg}", rj.job.title), false);
                        self.history.insert(0, entry(true, msg));
                    }
                },
                Err(e) => {
                    refresh = true;
                    // A cancel the user asked for is no error: it shows only in the history.
                    if !cancelled {
                        self.toast(format!("✖ {}: {e}", rj.job.title), true);
                    }
                    match rj.after {
                        After::OpenFile(p) | After::OpenWithDialog(p) => self.temp_copies.forget(&p),
                        // Not uploaded: asked again.
                        After::Uploaded(id, _) => {
                            if let Some(c) = self.temp_copies.get_mut(id) {
                                c.uploading = false;
                                c.asked = None;
                            }
                            self.history.insert(0, entry(false, e));
                        }
                        After::Refresh => self.history.insert(0, entry(false, e)),
                    }
                }
            }
        }
        self.history.truncate(100);
        if refresh {
            self.refresh_all();
        }
        self.schedule_jobs();
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The Transfers button's indicator while there are jobs: how far along the running ones
    /// are together (if known), and whether they are all paused.
    pub(super) fn transfers_gauge(&self) -> Option<(Option<f32>, bool)> {
        if self.jobs.is_empty() {
            return None;
        }
        let running: Vec<&ops::Progress> = self.jobs.iter().map(|j| &*j.job.progress).filter(|p| !p.is_queued()).collect();
        Some((ops::overall_fraction(&running), !running.is_empty() && running.iter().all(|p| p.is_paused())))
    }

    /// The Transfers window's content (in its tab).
    pub(super) fn transfers_view(&mut self, ui: &mut egui::Ui, side: usize) {
        let th = self.theme;
        let mut navigate: Option<Location> = None;
        let mut remove_queued: Option<usize> = None;
        let mut start_now: Option<usize> = None;
        let mut clear = false;
        let mut max_changed = false;
        // A click anywhere in it focuses the window.
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.active = side;
        }
        let running = self.jobs.iter().filter(|j| !j.job.progress.is_queued()).count();
        let queued = self.jobs.len() - running;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{running} {} · {queued} {}", t("running"), t("queued"))).color(th.weak));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                max_changed |= ui.add(egui::DragValue::new(&mut self.cfg.max_parallel).range(1..=8)).changed();
                ui.label(RichText::new(t("Run at the same time:")).size(12.0).color(th.weak));
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.jobs.is_empty() && self.history.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t("No tasks yet")).color(th.text));
                    ui.label(
                        RichText::new(t("Copy (F5), move (F6), compress or extract: progress shows up here while you keep browsing."))
                            .size(11.5)
                            .color(th.weak),
                    );
                });
            }
            for (i, j) in self.jobs.iter().enumerate() {
                let p = j.job.progress.clone();
                let is_q = p.is_queued();
                egui::Frame::NONE.fill(th.bg).corner_radius(10).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&j.job.title).font(egui::FontId::new(13.5, egui::FontFamily::Name("bold".into()))).color(th.strong()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::icon_button(ui, 24.0, true, th, |pp, r, c| close_x(pp, r, c)).on_hover_text(t("Cancel")).clicked() {
                                if is_q {
                                    remove_queued = Some(i);
                                } else {
                                    p.cancel();
                                }
                            }
                            if is_q {
                                if ui.small_button(t("Start now")).clicked() {
                                    start_now = Some(i);
                                }
                            } else if p.is_paused() {
                                if icons::icon_button(ui, 24.0, true, th, play_icon).on_hover_text(t("Resume")).clicked() {
                                    p.resume();
                                }
                            } else if icons::icon_button(ui, 24.0, true, th, pause_icon).on_hover_text(t("Pause")).clicked() {
                                p.pause();
                            }
                        });
                    });
                    // Progress bar
                    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 3.0, th.surface);
                    let bar_color = if p.is_paused() || is_q { th.weak } else { th.accent };
                    match p.fraction() {
                        Some(f) => {
                            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(r.width() * f, r.height())), 3.0, bar_color);
                        }
                        None if !is_q => {
                            let t0 = ui.input(|i| i.time) as f32;
                            let x = (t0 * 0.6).fract() * (r.width() + 80.0) - 80.0;
                            let seg = egui::Rect::from_min_max(
                                egui::pos2(r.left() + x.max(0.0), r.top()),
                                egui::pos2((r.left() + x + 80.0).min(r.right()), r.bottom()),
                            );
                            ui.painter().rect_filled(seg, 3.0, bar_color);
                        }
                        None => {}
                    }
                    let line = if is_q {
                        t("Waiting in queue…")
                    } else if p.is_paused() {
                        match p.fraction() {
                            Some(f) => format!("{} · {:.0}% · {}", t("Paused"), f * 100.0, p.amount_text()),
                            None => format!("{} · {}", t("Paused"), p.amount_text()),
                        }
                    } else {
                        let mut parts = vec![];
                        if let Some(f) = p.fraction() {
                            parts.push(format!("{:.0}%", f * 100.0));
                        }
                        parts.push(p.amount_text());
                        // Items are no bytes: no speed or time left for them.
                        if j.speed > 1.0 && !p.counts_items() {
                            parts.push(format!("{}/s", human_size(j.speed as u64)));
                            if p.total() > p.done() {
                                let eta = (p.total() - p.done()) as f64 / j.speed;
                                parts.push(format!("{} {}", fmt_duration(eta as f32), t("left")));
                            }
                        }
                        parts.join(" · ")
                    };
                    ui.label(RichText::new(line).size(12.0).color(th.text));
                    let cur = p.current();
                    if !cur.is_empty() && !is_q {
                        ui.add(egui::Label::new(RichText::new(cur).size(11.0).color(th.weak)).truncate());
                    }
                });
                ui.add_space(6.0);
            }
            if !self.history.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Finished")).size(11.5).color(th.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(t("Clear")).clicked() {
                            clear = true;
                        }
                    });
                });
                for h in &self.history {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                        let color = if h.ok {
                            th.accent
                        } else if h.cancelled {
                            th.weak
                        } else {
                            th.danger
                        };
                        ui.painter().circle_filled(r.center(), 4.0, color);
                        ui.label(RichText::new(&h.title).color(th.text));
                        let amount = if h.items { format!("{} {}", h.bytes, t("items")) } else { human_size(h.bytes) };
                        let detail = if h.ok {
                            format!("{amount} · {} · {}", fmt_duration(h.secs), h.at)
                        } else if h.cancelled {
                            format!("{} · {amount} · {}", t("Cancelled"), h.at)
                        } else {
                            format!("{} · {}", h.msg, h.at)
                        };
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(d) = &h.dest {
                                if ui.small_button(t("Show")).on_hover_text(t("Open the destination")).clicked() {
                                    navigate = Some(d.clone());
                                }
                            }
                            ui.add(egui::Label::new(RichText::new(detail).size(11.5).color(if h.ok || h.cancelled { th.weak } else { th.danger })).truncate());
                        });
                    });
                }
            }
        });
        if let Some(i) = start_now {
            if let Some(j) = self.jobs.get_mut(i) {
                j.job.progress.release();
                j.started_running = Some(Instant::now());
            }
        }
        if let Some(i) = remove_queued {
            if let Some(j) = self.jobs.get(i) {
                j.job.progress.cancel();
            }
        }
        if clear {
            self.history.clear();
        }
        if max_changed {
            self.save_cfg();
            self.schedule_jobs();
        }
        if let Some(loc) = navigate {
            self.navigate_active(loc);
        }
    }

    pub(super) fn start_transfer(&mut self, src_loc: &Location, items: Vec<Entry>, dst_loc: &Location, mv: bool, policy: Conflict) {
        let (Some(src), Some(dst_ep)) = (self.endpoint(src_loc), self.endpoint(dst_loc)) else { return };
        let list: Vec<ops::Item> = items.iter().map(|e| ops::Item { name: e.name.clone(), is_dir: e.is_dir, size: e.size }).collect();
        if list.is_empty() {
            return;
        }
        for side in 0..self.panes.len() {
            if self.panes[side].tab().loc == *src_loc {
                self.panes[side].tab_mut().marked.clear();
            }
        }
        let what = if list.len() == 1 { list[0].name.clone() } else { format!("{} {}", list.len(), t("item(s)")) };
        let title = format!("{} {what} → {}", if mv { t("Moving") } else { t("Copying") }, dst_loc.short_name());
        let sources = local_file_of(src_loc).into_iter().collect();
        self.spawn_job(title, After::Refresh, Some(dst_loc.clone()), sources, move |p| ops::transfer(src, list, dst_ep, mv, policy, p));
    }

    /// Items dropped onto a folder (panel, folder row, breadcrumb or sidebar).
    pub(super) fn handle_drop(&mut self, payload: &crate::ui::pane::DragPayload, target: Location) {
        if !crate::ui::pane::drop_allowed(payload, &target) {
            return;
        }
        let mv = self.shift_down;
        if matches!(payload.loc, Location::Archive { .. }) && mv {
            self.toast(t("Archives are read-only"), true);
            return;
        }
        if self.cfg.confirm_dnd {
            let dst = target.display(|id| self.remote_label(id));
            self.dialog_dst = Some(target);
            self.show_dialog(Dialog::Transfer { src: payload.loc.clone(), mv, items: payload.items.clone(), dst, policy: Conflict::Rename }, false);
        } else {
            self.start_transfer(&payload.loc, payload.items.clone(), &target, mv, Conflict::Rename);
        }
    }

    /// Make a sidebar row a drop target for the current drag.
    pub(super) fn sidebar_drop_target(&mut self, ui: &egui::Ui, rect: egui::Rect, target: Location) {
        let Some(pl) = crate::ui::pane::drag_payload(ui.ctx()) else { return };
        if !ui.rect_contains_pointer(rect) || !crate::ui::pane::drop_allowed(&pl, &target) {
            return;
        }
        let th = self.theme;
        ui.painter().rect_filled(rect, 7.0, th.accent.gamma_multiply(0.16));
        ui.painter().rect_stroke(rect, 7.0, egui::Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Inside);
        self.drag_target = Some(target.clone());
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(pl) = egui::DragAndDrop::take_payload::<crate::ui::pane::DragPayload>(ui.ctx()) {
                self.handle_drop(&pl, target);
            }
        }
    }

    /// Floating label that follows the pointer while dragging files.
    pub(super) fn drag_ghost(&mut self, ctx: &egui::Context) {
        let Some(pl) = crate::ui::pane::drag_payload(ctx) else { return };
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let th = self.theme;
        let what = if pl.items.len() == 1 { pl.items[0].name.clone() } else { format!("{} {}", pl.items.len(), t("item(s)")) };
        let (line, color) = match &self.drag_target {
            Some(tgt) => (format!("{} → {}", if self.shift_down { t("Move") } else { t("Copy") }, tgt.short_name()), th.accent),
            None => (t("Drop on a folder or the other panel"), th.weak),
        };
        egui::Area::new(egui::Id::new("drag-ghost")).order(egui::Order::Tooltip).fixed_pos(pos + egui::vec2(16.0, 12.0)).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(th.panel).corner_radius(9).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    if let Some(first) = pl.items.first() {
                        icons::entry_icon(ui.painter(), r, first, th);
                    }
                    if pl.items.len() > 1 {
                        let c = r.right_top() + egui::vec2(-1.0, 2.0);
                        ui.painter().circle_filled(c, 7.0, th.accent);
                        ui.painter().text(c, egui::Align2::CENTER_CENTER, pl.items.len().min(99).to_string(), egui::FontId::proportional(9.5), th.on_accent());
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(RichText::new(what).color(th.strong()));
                        ui.label(RichText::new(line).size(11.5).color(color));
                        if !self.shift_down && self.drag_target.is_some() {
                            ui.label(RichText::new(t("Hold Shift to move")).size(10.5).color(th.weak));
                        }
                    });
                });
            });
        });
        ctx.set_cursor_icon(if self.drag_target.is_some() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::NoDrop });
    }

    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        // Drop into the panel under the pointer (fall back to the active one).
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let a = pos.and_then(|p| self.pane_rects.iter().find(|(_, r)| r.contains(p)).map(|(i, _)| *i)).unwrap_or(self.active);
        // On a terminal: type the paths at the prompt, as other terminals do.
        if let Some(term) = &self.panes[a].tab().term {
            if self.dialog.is_some() {
                return;
            }
            let text = dropped.iter().map(|p| quote_dropped(&p.to_string_lossy())).collect::<Vec<_>>().join(" ");
            if !crate::terminal::view::is_typeable(&text) {
                self.toast(t("A dropped name has control characters: it is not typed into the terminal"), true);
                return;
            }
            term.paste(&format!("{text} "));
            self.active = a;
            self.focus_window();
            return;
        }
        // Settings, Tasks, Disk usage and tool windows show no folder to copy into.
        if !self.panes[a].tab().is_files() {
            self.toast(t("Drop files on a file window to copy them there"), true);
            return;
        }
        let loc = self.panes[a].tab().loc.clone();
        let Some(dst) = self.endpoint(&loc) else { return };
        // Group by parent folder (usually a single one).
        let mut groups: HashMap<PathBuf, Vec<ops::Item>> = HashMap::new();
        for p in dropped {
            let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { continue };
            let meta = std::fs::metadata(&p).ok();
            groups.entry(parent.to_path_buf()).or_default().push(ops::Item {
                name: name.to_string_lossy().to_string(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.map(|m| m.len()).unwrap_or(0),
            });
        }
        // Asked first when Settings say so, and always while a dialog is open (it waits
        // for that one).
        if self.cfg.confirm_dnd || self.dialog.is_some() {
            let dst_text = loc.display(|id| self.remote_label(id));
            for (parent, items) in groups {
                let items = items.into_iter().map(|i| Entry::new(i.name, i.is_dir, i.size, None)).collect();
                self.show_dialog(Dialog::Transfer { src: Location::Local(parent), mv: false, items, dst: dst_text.clone(), policy: Conflict::Rename }, false);
            }
            return;
        }
        for (parent, items) in groups {
            let d = dst.clone();
            let title = format!("{} {} {}", t("Copying"), items.len(), t("item(s)"));
            self.spawn_job(title, After::Refresh, Some(loc.clone()), vec![parent.clone()], move |p| {
                ops::transfer(Endpoint::Local(parent), items, d, false, Conflict::Rename, p)
            });
        }
    }
}

/// A dropped file's path as typed in a terminal: quoted when it has spaces or other
/// characters the shell would read.
fn quote_dropped(path: &str) -> String {
    if cfg!(windows) {
        if path.contains([' ', '&', '(', ')', '^', ';', ',', '\'', '%', '!']) {
            format!("\"{path}\"")
        } else {
            path.to_string()
        }
    } else {
        terminal::sh_quote(path)
    }
}

fn pause_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let w = r.width() * 0.22;
    p.rect_filled(egui::Rect::from_min_size(egui::pos2(r.center().x - w * 1.4, r.top() + 1.0), egui::vec2(w, r.height() - 2.0)), 1.0, c);
    p.rect_filled(egui::Rect::from_min_size(egui::pos2(r.center().x + w * 0.4, r.top() + 1.0), egui::vec2(w, r.height() - 2.0)), 1.0, c);
}

fn play_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let pts = vec![
        egui::pos2(r.left() + r.width() * 0.25, r.top()),
        egui::pos2(r.right() - r.width() * 0.1, r.center().y),
        egui::pos2(r.left() + r.width() * 0.25, r.bottom()),
    ];
    p.add(egui::Shape::convex_polygon(pts, c, egui::Stroke::NONE));
}

fn fmt_duration(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
