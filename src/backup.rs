//! Custom local backups: profiles of sources + exclude globs written to an archive file.
//! No cloud: backups are plain archive files that can be stored anywhere.
use crate::archive::{self, ArchiveFormat};
use crate::config::BackupProfile;
use crate::ops::Progress;
use crate::theme::Theme;
use crate::util::{es, Res};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "matrix_finder-backup.json";

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

pub fn build_globset(patterns: &[String]) -> Res<Option<GlobSet>> {
    let pats: Vec<&String> = patterns.iter().filter(|p| !p.trim().is_empty()).collect();
    if pats.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for p in pats {
        b.add(Glob::new(p.trim()).map_err(|e| format!("Bad pattern '{p}': {e}"))?);
    }
    b.build().map(Some).map_err(es)
}

fn file_name(profile: &BackupProfile, date: &str) -> String {
    let t = if profile.file_template.trim().is_empty() { "{name}-{date}" } else { profile.file_template.trim() };
    format!(
        "{}.{}",
        t.replace("{name}", &sanitize(&profile.name)).replace("{date}", date),
        profile.format.extension()
    )
}

/// Run a backup. Returns the path of the created archive.
pub fn run(profile: &BackupProfile, p: &Progress) -> Res<String> {
    if profile.sources.is_empty() {
        return Err("The backup profile has no source folders".into());
    }
    if profile.format.single_file() {
        return Err("Choose a container format (zip, 7z, tar.*) for backups".into());
    }
    let sources: Vec<PathBuf> = profile.sources.iter().map(PathBuf::from).collect();
    for s in &sources {
        if !s.exists() {
            return Err(format!("Source does not exist: {}", s.display()));
        }
    }
    let excludes = build_globset(&profile.excludes)?;
    let dest_dir = PathBuf::from(&profile.destination);
    std::fs::create_dir_all(&dest_dir).map_err(es)?;
    let date = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dest = dest_dir.join(file_name(profile, &date));
    for s in &sources {
        if dest.starts_with(s) && profile.excludes.is_empty() {
            return Err("The destination is inside a source folder; add an exclude pattern for it".into());
        }
    }
    let manifest = serde_json::json!({
        "app": "Matrix Finder",
        "version": env!("CARGO_PKG_VERSION"),
        "profile": profile.name,
        "created": chrono::Local::now().to_rfc3339(),
        "sources": profile.sources,
        "excludes": profile.excludes,
        "format": profile.format.extension(),
        "host": std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")).unwrap_or_default(),
    });
    let extra = vec![(MANIFEST.to_string(), serde_json::to_vec_pretty(&manifest).map_err(es)?)];
    archive::create(&sources, excludes.as_ref(), &dest, profile.format, profile.level, &extra, p)?;
    if profile.keep_last > 0 {
        rotate(profile, &dest_dir, &dest);
    }
    Ok(dest.to_string_lossy().to_string())
}

/// Whether `s` is a date as written into backup names (`%Y%m%d-%H%M%S`).
fn is_date_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15 && b[8] == b'-' && b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// Remove older archives of the same profile beyond `keep_last`. Only names that are
/// exactly this profile's template with a date in it count: other profiles can share
/// the folder and the prefix (`Docs-…` and `Docs-old-…`), and so can the user's own files.
fn rotate(profile: &BackupProfile, dir: &Path, newest: &Path) {
    let sample = file_name(profile, "\u{0}");
    let Some((prefix, suffix)) = sample.split_once('\u{0}') else { return };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let date = n.strip_prefix(prefix)?.strip_suffix(suffix)?;
            is_date_stamp(date).then(|| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in found.into_iter().skip(profile.keep_last) {
        if p != newest {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub enum BackupAction {
    Run(BackupProfile),
    Restore(PathBuf, PathBuf),
    Changed,
    Export,
    Import,
}

#[derive(Default)]
pub struct BackupUi {
    pub open: bool,
    selected: usize,
    new_source: String,
    new_exclude: String,
    restore_file: String,
    restore_dest: String,
}

impl BackupUi {
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        profiles: &mut Vec<BackupProfile>,
        current_dir: Option<&Path>,
        theme: &Theme,
        t: &dyn Fn(&str) -> String,
    ) -> Option<BackupAction> {
        let mut action = None;
        let mut open = self.open;
        egui::Window::new(t("Backup"))
            .open(&mut open)
            .collapsible(false)
            .default_size([860.0, 560.0])
            .max_height(720.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(t("Local backups to an archive file (no cloud).")).color(theme.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(format!("⬇ {}", t("Import profiles…"))).clicked() {
                            action = Some(BackupAction::Import);
                        }
                        if ui.button(format!("⬆ {}", t("Export profiles…"))).clicked() {
                            action = Some(BackupAction::Export);
                        }
                    });
                });
                ui.separator();
                ui.horizontal_top(|ui| {
                    // Profile list
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        ui.strong(t("Profiles"));
                        egui::ScrollArea::vertical().id_salt("bk-list").max_height(330.0).show(ui, |ui| {
                            for (i, p) in profiles.iter().enumerate() {
                                if ui.selectable_label(self.selected == i, &p.name).clicked() {
                                    self.selected = i;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("➕").on_hover_text(t("New profile")).clicked() {
                                let mut p = BackupProfile::default();
                                if let Some(d) = current_dir {
                                    p.sources.push(d.to_string_lossy().to_string());
                                }
                                profiles.push(p);
                                self.selected = profiles.len() - 1;
                                action = Some(BackupAction::Changed);
                            }
                            if ui.add_enabled(!profiles.is_empty(), egui::Button::new("📋")).on_hover_text(t("Duplicate")).clicked() {
                                let mut p = profiles[self.selected].clone();
                                p.name = format!("{} (2)", p.name);
                                p.last_run = None;
                                profiles.push(p);
                                self.selected = profiles.len() - 1;
                                action = Some(BackupAction::Changed);
                            }
                            if ui.add_enabled(!profiles.is_empty(), egui::Button::new("🗑")).on_hover_text(t("Delete")).clicked() {
                                profiles.remove(self.selected);
                                self.selected = self.selected.saturating_sub(1);
                                action = Some(BackupAction::Changed);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        if profiles.is_empty() {
                            ui.label(t("No backup profiles yet. Press ➕ to create one."));
                            return;
                        }
                        self.selected = self.selected.min(profiles.len() - 1);
                        let p = &mut profiles[self.selected];
                        let before = p.clone();
                        egui::ScrollArea::vertical().id_salt("bk-form-scroll").max_height(400.0).show(ui, |ui| {
                        egui::Grid::new("bk-form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                            ui.label(t("Name"));
                            ui.text_edit_singleline(&mut p.name);
                            ui.end_row();

                            ui.label(t("Sources"));
                            ui.vertical(|ui| {
                                let mut remove = None;
                                for (i, s) in p.sources.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("✖").clicked() {
                                            remove = Some(i);
                                        }
                                        ui.label(s);
                                    });
                                }
                                if let Some(i) = remove {
                                    p.sources.remove(i);
                                }
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.new_source).hint_text(t("Folder or file path")).desired_width(240.0));
                                    if ui.button(t("Add")).clicked() && !self.new_source.trim().is_empty() {
                                        p.sources.push(self.new_source.trim().to_string());
                                        self.new_source.clear();
                                    }
                                    if ui.button(t("Browse…")).clicked() {
                                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                            p.sources.push(d.to_string_lossy().to_string());
                                        }
                                    }
                                    if let Some(d) = current_dir {
                                        if ui.button(t("Current folder")).clicked() {
                                            p.sources.push(d.to_string_lossy().to_string());
                                        }
                                    }
                                });
                            });
                            ui.end_row();

                            ui.label(t("Exclude (glob)"));
                            ui.vertical(|ui| {
                                let mut remove = None;
                                ui.horizontal_wrapped(|ui| {
                                    for (i, s) in p.excludes.iter().enumerate() {
                                        if ui.small_button(format!("{s}  ✖")).clicked() {
                                            remove = Some(i);
                                        }
                                    }
                                });
                                if let Some(i) = remove {
                                    p.excludes.remove(i);
                                }
                                ui.horizontal(|ui| {
                                    let r = ui.add(egui::TextEdit::singleline(&mut self.new_exclude).hint_text("**/cache/**, *.log").desired_width(240.0));
                                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    if (ui.button(t("Add")).clicked() || enter) && !self.new_exclude.trim().is_empty() {
                                        for part in self.new_exclude.split(',') {
                                            if !part.trim().is_empty() {
                                                p.excludes.push(part.trim().to_string());
                                            }
                                        }
                                        self.new_exclude.clear();
                                    }
                                });
                                if let Err(e) = build_globset(&p.excludes) {
                                    ui.colored_label(theme.danger, e);
                                }
                            });
                            ui.end_row();

                            ui.label(t("Destination"));
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.destination).desired_width(300.0));
                                if ui.button(t("Browse…")).clicked() {
                                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                        p.destination = d.to_string_lossy().to_string();
                                    }
                                }
                            });
                            ui.end_row();

                            ui.label(t("Format"));
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("bk-fmt").selected_text(p.format.label()).show_ui(ui, |ui| {
                                    for f in ArchiveFormat::ALL.iter().filter(|f| !f.single_file()) {
                                        ui.selectable_value(&mut p.format, *f, f.label());
                                    }
                                });
                                ui.label(t("Level"));
                                ui.add(egui::Slider::new(&mut p.level, 1..=9));
                            });
                            ui.end_row();

                            ui.label(t("File name"));
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut p.file_template).desired_width(200.0));
                                ui.label(egui::RichText::new(format!("→ {}", file_name(p, "20250101-120000"))).color(theme.weak));
                            });
                            ui.end_row();

                            ui.label(t("Keep last"));
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut p.keep_last).range(0..=999));
                                ui.label(egui::RichText::new(t("(0 = keep all)")).color(theme.weak));
                            });
                            ui.end_row();
                        });
                        });
                        if let Some(l) = &p.last_run {
                            ui.label(egui::RichText::new(format!("{}: {l}", t("Last run"))).color(theme.weak));
                        }
                        ui.add_space(6.0);
                        if ui
                            .add_enabled(!p.sources.is_empty(), egui::Button::new(egui::RichText::new(format!("▶ {}", t("Run backup now"))).strong()))
                            .clicked()
                        {
                            p.last_run = Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string());
                            action = Some(BackupAction::Run(p.clone()));
                        }
                        if *p != before && action.is_none() {
                            action = Some(BackupAction::Changed);
                        }
                    });
                });
                ui.separator();
                ui.strong(t("Restore"));
                ui.horizontal(|ui| {
                    ui.label(t("Backup file"));
                    ui.add(egui::TextEdit::singleline(&mut self.restore_file).desired_width(280.0));
                    if ui.button("…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().pick_file() {
                            self.restore_file = f.to_string_lossy().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("Restore to"));
                    ui.add(egui::TextEdit::singleline(&mut self.restore_dest).desired_width(280.0));
                    if ui.button("…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().pick_folder() {
                            self.restore_dest = f.to_string_lossy().to_string();
                        }
                    }
                    let ok = !self.restore_file.is_empty() && !self.restore_dest.is_empty();
                    if ui.add_enabled(ok, egui::Button::new(format!("⟲ {}", t("Restore")))).clicked() {
                        action = Some(BackupAction::Restore(PathBuf::from(&self.restore_file), PathBuf::from(&self.restore_dest)));
                    }
                });
            });
        self.open = open;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_with_excludes_and_rotation() {
        let base = std::env::temp_dir().join(format!("matrix_finder-bk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("proj");
        std::fs::create_dir_all(src.join("node_modules/x")).unwrap();
        std::fs::write(src.join("keep.txt"), "k").unwrap();
        std::fs::write(src.join("junk.tmp"), "j").unwrap();
        std::fs::write(src.join("node_modules/x/big.js"), "b").unwrap();
        let profile = BackupProfile {
            name: "Test Proj".into(),
            sources: vec![src.to_string_lossy().to_string()],
            destination: base.join("out").to_string_lossy().to_string(),
            keep_last: 1,
            ..Default::default()
        };
        let out = run(&profile, &Progress::default()).unwrap();
        let entries: Vec<String> = archive::list(Path::new(&out)).unwrap().into_iter().map(|e| e.path).collect();
        assert!(entries.contains(&"proj/keep.txt".to_string()), "{entries:?}");
        assert!(entries.contains(&MANIFEST.to_string()));
        assert!(!entries.iter().any(|e| e.contains("node_modules")), "{entries:?}");
        assert!(!entries.iter().any(|e| e.ends_with(".tmp")), "{entries:?}");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let out2 = run(&profile, &Progress::default()).unwrap();
        assert_ne!(out, out2);
        assert!(!Path::new(&out).exists(), "rotation should delete the older backup");
        assert!(Path::new(&out2).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rotation_only_touches_this_profiles_dated_files() {
        let dir = std::env::temp_dir().join(format!("matrix_finder-bk-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let profile = BackupProfile { name: "Docs".into(), keep_last: 1, ..Default::default() };
        let names = [
            "Docs-20200101-000000.zip",
            "Docs-20210101-000000.zip",
            "Docs-old-20200101-000000.zip",
            "Docs-notes.zip",
            "Docs-20200101.zip",
        ];
        let base = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
        for (i, n) in names.iter().enumerate() {
            let f = std::fs::File::create(dir.join(n)).unwrap();
            f.set_modified(base + std::time::Duration::from_secs(i as u64)).unwrap();
        }
        let newest = dir.join("Docs-20250101-120000.zip");
        std::fs::write(&newest, "").unwrap();
        rotate(&profile, &dir, &newest);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, ["Docs-20200101.zip", "Docs-20250101-120000.zip", "Docs-notes.zip", "Docs-old-20200101-000000.zip"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
