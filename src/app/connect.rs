//! Server connections (FTP, SFTP) and drives: connecting, disconnecting, ejecting.
use super::*;

impl App {
    pub(super) fn open_connect_url(&mut self, url: &str) {
        let (proto, rest) = if let Some(r) = url.strip_prefix("sftp://") {
            (Protocol::Sftp, r)
        } else if let Some(r) = url.strip_prefix("ftp://") {
            (Protocol::Ftp, r)
        } else {
            return;
        };
        let (auth_host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, String::new()),
        };
        let (user, hostport) = match auth_host.rsplit_once('@') {
            Some((u, h)) => (u.to_string(), h),
            None => (String::new(), auth_host),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(proto.default_port())),
            None => (hostport.to_string(), proto.default_port()),
        };
        // Reuse a saved profile (key file / password) when one matches.
        let mut profile = self
            .cfg
            .connections
            .iter()
            .find(|c| c.protocol == proto && c.host == host && c.port == port && c.user == user)
            .cloned()
            .unwrap_or(ConnectionProfile { name: format!("{user}@{host}"), protocol: proto, host, port, user, ..Default::default() });
        profile.initial_path = path;
        self.show_dialog(Dialog::Connect { profile, save: false, save_password: false }, true);
    }

    pub(super) fn open_connect_dialog(&mut self, profile: Option<ConnectionProfile>) {
        self.show_dialog(Dialog::Connect { profile: profile.unwrap_or_default(), save: true, save_password: false }, true);
    }

    pub(super) fn start_connect(&mut self, profile: ConnectionProfile, save: bool, save_password: bool) {
        if save {
            let mut stored = profile.clone();
            if !save_password {
                stored.password.clear();
            }
            if stored.name.trim().is_empty() {
                stored.name = format!("{}@{}", stored.user, stored.host);
            }
            if let Some(existing) = self.cfg.connections.iter_mut().find(|c| c.name == stored.name) {
                *existing = stored;
            } else {
                self.cfg.connections.push(stored);
            }
            self.save_cfg();
        }
        self.spawn_connect(profile, save, None);
    }

    /// Connect in the background. `trusted_key` is a host key fingerprint the user accepted.
    pub(super) fn spawn_connect(&mut self, profile: ConnectionProfile, save: bool, trusted_key: Option<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let p = profile.clone();
        std::thread::spawn(move || {
            let r = remote::connect(&p, trusted_key.as_deref()).map(|mut fs| {
                let home = if p.initial_path.is_empty() { fs.home() } else { p.initial_path.clone() };
                (fs, home)
            });
            let _ = tx.send(r);
        });
        self.connecting = Some((profile, save, rx));
    }

    pub(super) fn poll_connect(&mut self) {
        // Not while a dialog is open: it would lose the folder it acts on, or be replaced.
        if self.dialog.is_some() {
            return;
        }
        let Some((profile, _, rx)) = &self.connecting else { return };
        let Ok(r) = rx.try_recv() else { return };
        let profile = profile.clone();
        self.connecting = None;
        match r {
            Ok((fs, home)) => {
                let id = self.next_remote;
                self.next_remote += 1;
                let label = format!("{}://{}@{}:{}", profile.protocol.scheme(), profile.user, profile.host, profile.port);
                self.remotes.insert(id, (label.clone(), Arc::new(Mutex::new(fs))));
                self.toast(format!("{} {label}", t("Connected to")), false);
                let home = if home.is_empty() { "/".to_string() } else { home };
                self.navigate_active(Location::Remote { id, path: home });
            }
            Err(remote::ConnectError::UnknownHostKey { fingerprint }) => {
                self.show_dialog(Dialog::TrustHostKey { profile, fingerprint }, false);
            }
            Err(remote::ConnectError::Other(e)) => {
                self.toast(format!("{}: {e}", t("Connection failed")), true);
                self.show_dialog(Dialog::Connect { profile, save: false, save_password: false }, false);
            }
        }
    }

    pub(super) fn disconnect(&mut self, id: u64) {
        self.remotes.remove(&id);
        let home = Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        let c = lc!(self);
        for p in all_panes!(self) {
            if matches!(p.tab().loc, Location::Remote { id: rid, .. } if rid == id) {
                p.tab_mut().navigate(home.clone(), &c);
            }
        }
    }

    pub(super) fn connect_saved(&mut self, c: ConnectionProfile) {
        if c.password.is_empty() && c.key_file.is_empty() {
            self.open_connect_dialog(Some(c));
        } else {
            self.start_connect(c, false, false);
        }
    }

    /// Ejects a drive in the background. Refused while a transfer uses it; previews of
    /// its files are closed first, and windows showing it go home once it is gone.
    pub(super) fn eject_drive(&mut self, d: crate::drives::Drive) {
        let mount = d.mount.clone();
        if !d.ejectable || self.ejecting.iter().any(|e| e.0 == mount) {
            return;
        }
        let name = d.name.clone();
        if self.jobs.iter().any(|j| j.paths.iter().any(|p| crate::drives::on_drive(p, &mount))) {
            self.toast(format!("{} {name}: {}", t("Could not eject"), t("A task is using this drive. Wait for it to finish, or cancel it.")), true);
            return;
        }
        let hashing = all_panes!(self).any(|p| p.tab().checksum.as_ref().is_some_and(|c| c.reads_under(&mount)));
        if hashing {
            self.toast(format!("{} {name}: {}", t("Could not eject"), t("A Checksum tab is reading files on this drive. Wait for it to finish, or close the tab.")), true);
            return;
        }
        // Let go of what this app reads there.
        let shown = mount.to_string_lossy().to_string();
        if self.quick.as_ref().is_some_and(|q| q.key.contains(&shown)) {
            self.quick = None;
        }
        if self.viewer.as_ref().is_some_and(|q| q.key.contains(&shown)) {
            self.viewer = None;
        }
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            if local_file_of(&tab.loc).is_some_and(|p| crate::drives::on_drive(&p, &mount)) {
                tab.dir_sizes = None;
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::drives::eject(&d));
        });
        self.ejecting.push((mount, name, rx));
    }

    /// Windows showing the drive at `mount` go to the home folder; with `only_gone`, only
    /// those whose folder no longer exists (the drive went away by itself).
    fn leave_drive(&mut self, mount: &Path, only_gone: bool) {
        let home = Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        let lc = lc!(self);
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            if !tab.is_files() {
                continue;
            }
            let Some(path) = local_file_of(&tab.loc) else { continue };
            if crate::drives::on_drive(&path, mount) && (!only_gone || !path.exists()) {
                tab.navigate(home.clone(), &lc);
            }
        }
    }

    /// New drive lists from the watcher, and ejects that finished.
    pub(super) fn poll_drives(&mut self, ctx: &egui::Context) {
        if let Some(new) = self.drive_watch.poll() {
            let gone = crate::drives::removed(&self.drives, &new);
            self.drives = new;
            for mount in gone {
                self.leave_drive(&mount, true);
            }
        }
        if self.ejecting.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        let mut done = Vec::new();
        self.ejecting.retain(|(mount, name, rx)| match rx.try_recv() {
            Ok(r) => {
                done.push((mount.clone(), name.clone(), r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                done.push((mount.clone(), name.clone(), Err(t("The drive could not be ejected."))));
                false
            }
        });
        for (mount, name, result) in done {
            match result {
                Ok(()) => {
                    self.drives.retain(|d| d.mount != mount);
                    self.leave_drive(&mount, false);
                    self.toast(format!("{}: {name}", t("Ejected")), false);
                }
                Err(e) => self.toast(format!("{} {name}: {e}", t("Could not eject")), true),
            }
            self.drive_watch.refresh();
        }
    }
}
