//! Terminals: opening them in folders or with a profile, their events, and the AI
//! coding agents of the AI button.
use super::*;

impl App {
    pub(super) fn open_terminal(&mut self) {
        let a = self.active;
        let dir = match &self.panes[a].tab().loc {
            Location::Local(p) => p.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Location::Remote { .. } => {
                self.toast(t("Terminal opens only for local folders"), true);
                return;
            }
        };
        let idx = self.cfg.default_terminal;
        self.launch_profile(idx, &dir);
    }

    /// Open a terminal in each selected folder of the active window; several ask first,
    /// so a stray click never opens a pile of terminals.
    pub(super) fn terminal_in_selection(&mut self) {
        let tab = self.panes[self.active].tab();
        let Location::Local(base) = &tab.loc else {
            self.toast(t("Terminal opens only for local folders"), true);
            return;
        };
        let dirs: Vec<PathBuf> = tab.selection().iter().filter(|e| e.is_dir && !e.is_parent).map(|e| base.join(&e.name)).collect();
        // Each one is a terminal (a tab and a shell): a few at a time, not a whole drive.
        if dirs.len() > MAX_TERMINALS {
            self.toast(t("Open terminals in at most {n} folders at once").replace("{n}", &MAX_TERMINALS.to_string()), true);
            return;
        }
        match dirs.as_slice() {
            [] => {}
            [one] => {
                let one = one.clone();
                self.launch_profile(self.cfg.default_terminal, &one);
            }
            _ => self.show_dialog(Dialog::OpenTerminals { dirs }, false),
        }
    }

    /// Open a terminal in the active folder with a profile (`None` = default).
    pub(super) fn open_terminal_profile(&mut self, idx: Option<usize>) {
        let i = idx.unwrap_or(self.cfg.default_terminal);
        let a = self.active;
        let dir = match &self.panes[a].tab().loc {
            Location::Local(p) => p.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Location::Remote { .. } => {
                self.toast(t("Terminal opens only for local folders"), true);
                return;
            }
        };
        self.launch_profile(i, &dir);
    }

    pub(super) fn launch_profile(&mut self, idx: usize, dir: &Path) {
        let Some(p) = self.cfg.terminal_profiles.get(idx).cloned() else { return };
        if p.terminal == TerminalKind::BuiltIn {
            self.new_terminal_here(Some(idx), Some(dir.to_path_buf()));
            return;
        }
        if let Err(e) = terminal::open_profile(&p, dir) {
            self.toast(format!("{}: {e}", p.name), true);
        }
    }

    pub(super) fn handle_term_response(&mut self, side: usize, r: crate::termview::TermResponse) {
        if r.focus && self.active != side {
            self.active = side;
            self.quick = None;
        }
        if r.font_delta != 0.0 {
            let (sizes, old) = (crate::fonts::TERM_SIZES, self.cfg.term_font_size);
            self.cfg.term_font_size = (old + r.font_delta).clamp(sizes.start().min(old), sizes.end().max(old));
            self.term_font_saved_at = Some(Instant::now());
        }
        if let Some(dir) = r.open_dir {
            let (at, group) = self.next_slot();
            self.new_files_tab(at, group, Some(Location::Local(dir)));
        }
        if let Some((dir, terminal)) = r.split {
            self.split_pane(side, dir, terminal);
        }
        if r.restart {
            let tab = self.panes[side].tab();
            if let Some(term) = &tab.term {
                let (dir, argv, th, startup) = (term.cwd.clone(), term.argv.clone(), term.theme, term.startup.clone());
                let dir = if dir.is_dir() { dir } else { dirs::home_dir().unwrap_or_default() };
                match TermSession::spawn(&self.egui_ctx, &dir, argv, self.cfg.term_scrollback) {
                    Ok(mut s) => {
                        s.theme = th;
                        s.run_at_start(startup);
                        self.panes[side].tab_mut().term = Some(Box::new(s));
                    }
                    Err(e) => self.toast(format!("{}: {e}", t("Cannot start terminal")), true),
                }
            }
        }
        if r.close {
            self.active = side;
            self.close_current();
        }
    }

    /// The agent the AI button in the terminals' info bars starts (`None`: no button),
    /// and every agent found. Starts looking for them the first time a terminal is shown.
    pub(super) fn ai_button(&mut self) -> (Option<crate::agents::Found>, Vec<crate::agents::Found>) {
        if !self.cfg.term_agents {
            return (None, Vec::new());
        }
        if self.agents.is_none() && self.panes.iter().any(|p| p.tab().is_terminal()) {
            self.find_agents(false);
        }
        let found = self.agents.clone().unwrap_or_default();
        (crate::agents::pick(&found, &self.cfg.term_agent).cloned(), found)
    }

    /// Look for the installed AI coding agents (again, when `again`).
    pub(super) fn find_agents(&mut self, again: bool) {
        if self.agents_rx.is_some() || (self.agents.is_some() && !again) {
            return;
        }
        // Tests (and README screenshots) do not depend on what this computer has.
        if cfg!(test) {
            self.agents.get_or_insert_with(Vec::new);
            return;
        }
        self.agents_rx = Some(crate::agents::detect_in_background(&self.egui_ctx));
    }

    pub(super) fn poll_agents(&mut self) {
        if let Some(found) = self.agents_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.agents = Some(found);
            self.agents_rx = None;
        }
    }

    pub(super) fn agent_act(&mut self, side: usize, act: crate::agents::AgentAct) {
        use crate::agents::AgentAct;
        match act {
            AgentAct::Run(id) => {
                let Some(run) = self.agents.as_ref().and_then(|v| v.iter().find(|f| f.id == id)).map(|f| f.run.clone()) else { return };
                self.active = side;
                if let Some(term) = self.panes[side].tab_mut().term.as_mut() {
                    term.want_focus = true;
                    // Nothing is sent unless the shell waits at its prompt (the button is
                    // greyed out otherwise).
                    term.run_program(&run);
                }
            }
            AgentAct::Choose(id) => {
                self.cfg.term_agent = id.to_string();
                self.save_cfg();
            }
            AgentAct::HideButton => {
                self.cfg.term_agents = false;
                self.save_cfg();
                self.toast(t("The AI button is hidden. Settings → Terminal shows it again."), false);
            }
            AgentAct::Settings => self.open_settings(SettingsPage::Terminal),
        }
    }

    /// Poll every terminal (also in hidden tabs): titles, folder, exit.
    pub(super) fn poll_terminals(&mut self, ctx: &egui::Context) {
        // A pinch changes the text size many times a second: save once it settles.
        if let Some(at) = self.term_font_saved_at {
            if at.elapsed() >= Duration::from_millis(800) {
                self.term_font_saved_at = None;
                self.save_cfg();
            } else {
                ctx.request_repaint_after(Duration::from_millis(800) - at.elapsed());
            }
        }
        let mut done: Vec<u64> = Vec::new();
        let close = self.cfg.term_close_on_exit;
        for pane in all_panes!(self) {
            let tab = pane.tab_mut();
            let Some(t) = &mut tab.term else { continue };
            t.poll(ctx);
            if tab.loc.local_path() != Some(t.cwd.as_path()) {
                tab.loc = Location::Local(t.cwd.clone());
            }
            if t.exited == Some(0) && close {
                done.push(tab.uid);
            }
        }
        for view in done {
            if let Some(side) = self.panes.iter().position(|p| p.tab().uid == view) {
                // In the shown tab: close its window (or the tab, if it is the only one),
                // leaving the focus where it is.
                self.close_pane(side);
                self.closed_tabs.pop();
            } else if self.take_window(view).is_none() {
                // The only window of a hidden tab: close that tab.
                if let Some(w) = self.strip.tabs.iter().find(|w| w.panes.iter().any(|p| p.tab().uid == view)) {
                    let uid = w.uid;
                    self.close_tabs(&[uid]);
                    self.closed_tabs.pop();
                }
            }
        }
    }
}

/// The most terminals "Open terminals in N folders" opens at once.
const MAX_TERMINALS: usize = 20;
