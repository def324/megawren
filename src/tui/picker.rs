use super::model::MAX_SELECTION;
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const PAGE: usize = 256;
#[derive(Clone)]
pub struct LocalEntry {
    pub path: PathBuf,
    pub name: String,
    pub folder: bool,
    pub size: i64,
    pub parent: bool,
}
pub struct Picker {
    pub viewport: std::cell::Cell<usize>,
    pub directory: PathBuf,
    pub entries: Vec<LocalEntry>,
    names: Vec<(bool, String)>,
    pub offset: usize,
    pub more: bool,
    pub cursor: usize,
    pub selected: BTreeSet<PathBuf>,
    pub show_hidden: bool,
    pub directory_only: bool,
    pub button_focus: bool,
    pub conflict: bool,
    pub error: Option<String>,
    pub location_input: Option<String>,
}
impl Picker {
    pub fn new(path: &Path, directory_only: bool) -> Result<Self> {
        let mut picker = Self {
            viewport: std::cell::Cell::new(0),
            directory: fs::canonicalize(path)?,
            entries: Vec::new(),
            names: Vec::new(),
            offset: 0,
            more: false,
            cursor: 0,
            selected: BTreeSet::new(),
            show_hidden: false,
            directory_only,
            button_focus: false,
            conflict: false,
            error: None,
            location_input: None,
        };
        picker.reload()?;
        Ok(picker)
    }
    fn reload(&mut self) -> Result<()> {
        let mut names = Vec::new();
        for item in fs::read_dir(&self.directory)? {
            let item = item?;
            let Ok(name) = item.file_name().into_string() else {
                continue;
            };
            if !self.show_hidden && name.starts_with('.') {
                continue;
            }
            let kind = item.file_type()?;
            if kind.is_dir() || (!self.directory_only && kind.is_file()) {
                names.push((kind.is_dir(), name));
            }
        }
        names.sort_by_cached_key(|(folder, name)| (!folder, name.to_lowercase(), name.clone()));
        self.names = names;
        self.show_page();
        Ok(())
    }
    fn show_page(&mut self) {
        let mut entries = Vec::new();
        if let Some(parent) = self.directory.parent() {
            entries.push(LocalEntry {
                path: parent.to_path_buf(),
                name: "..".into(),
                folder: true,
                size: 0,
                parent: true,
            });
        }
        for (folder, name) in self.names.iter().skip(self.offset).take(PAGE) {
            let path = self.directory.join(name);
            let size = if *folder {
                0
            } else {
                fs::symlink_metadata(&path)
                    .ok()
                    .filter(|m| m.is_file())
                    .and_then(|m| i64::try_from(m.len()).ok())
                    .unwrap_or(-1)
            };
            entries.push(LocalEntry {
                path,
                name: name.clone(),
                folder: *folder,
                size,
                parent: false,
            });
        }
        self.more = self.names.len() > self.offset + PAGE;
        self.entries = entries;
        self.viewport.set(0);
        self.cursor =
            usize::from(self.entries.first().is_some_and(|e| e.parent) && self.entries.len() > 1);
        self.error = None;
    }
    pub fn page(&mut self, forward: bool) -> Result<()> {
        if forward && self.more {
            self.offset += PAGE;
        } else if !forward && self.offset > 0 {
            self.offset -= PAGE;
        } else {
            self.move_by(if forward { 12 } else { -12 });
            return Ok(());
        }
        self.show_page();
        Ok(())
    }
    pub fn navigate(&mut self, path: PathBuf) -> Result<()> {
        let previous = self.directory.clone();
        self.directory = fs::canonicalize(path)?;
        let offset = std::mem::take(&mut self.offset);
        if let Err(e) = self.reload() {
            self.directory = previous;
            self.offset = offset;
            return Err(e);
        }
        Ok(())
    }
    pub fn open(&mut self) -> Result<()> {
        if self.button_focus {
            return Ok(());
        }
        if let Some(entry) = self.entries.get(self.cursor) {
            if entry.parent {
                return self.parent();
            }
            if entry.folder {
                return self.navigate(entry.path.clone());
            }
            return self.toggle();
        }
        Ok(())
    }
    pub fn parent(&mut self) -> Result<()> {
        let child = self.directory.clone();
        if let Some(parent) = self.directory.parent() {
            self.navigate(parent.to_path_buf())?;
            if let Some(index) = self
                .names
                .iter()
                .position(|(_, name)| child.file_name() == Some(name.as_ref()))
            {
                self.offset = index / PAGE * PAGE;
                self.show_page();
            }
            self.cursor = self
                .entries
                .iter()
                .position(|e| e.path == child)
                .unwrap_or(self.cursor);
        }
        Ok(())
    }
    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.offset = 0;
        self.reload()
    }
    pub fn toggle(&mut self) -> Result<()> {
        if self.directory_only {
            return Ok(());
        }
        let entry = self.entries.get(self.cursor).context("Nothing selected")?;
        if entry.parent {
            return Ok(());
        }
        if self.selected.remove(&entry.path) {
            return Ok(());
        }
        if self.selected.iter().any(|p| entry.path.starts_with(p)) {
            return Ok(());
        }
        if entry.folder {
            self.selected.retain(|p| !p.starts_with(&entry.path));
        }
        ensure!(
            self.selected.len() < MAX_SELECTION,
            "Select up to {MAX_SELECTION} items at a time"
        );
        self.selected.insert(entry.path.clone());
        Ok(())
    }
    pub fn check_download_names(&mut self, entries: &[super::model::Entry]) {
        self.conflict = entries
            .iter()
            .any(|entry| std::fs::symlink_metadata(self.directory.join(&entry.name)).is_ok());
    }
    pub fn sources(&self) -> Result<Vec<PathBuf>> {
        ensure!(
            !self.selected.is_empty(),
            "Select files or folders with Space first"
        );
        Ok(self.selected.iter().cloned().collect())
    }
    pub fn move_by(&mut self, delta: isize) {
        self.button_focus = false;
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }
    pub fn edit_location(&mut self) {
        self.location_input = Some(self.directory.display().to_string());
        self.button_focus = false;
    }
    pub fn enter_location(&mut self) -> Result<()> {
        let Some(text) = self.location_input.as_ref() else {
            return Ok(());
        };
        let path = if text == "~" || text.starts_with("~/") {
            let home = dirs::home_dir().context("Cannot determine home directory")?;
            home.join(text.strip_prefix("~/").unwrap_or(""))
        } else {
            self.directory.join(text)
        };
        self.navigate(path)?;
        self.location_input = None;
        Ok(())
    }
    pub fn paste_location(&mut self, text: &str) {
        if let Some(input) = self.location_input.as_mut() {
            for c in text.chars().filter(|c| !c.is_control()) {
                if input.len() + c.len_utf8() > 32768 {
                    break;
                }
                input.push(c);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_picker_directories_are_browsable_with_bounded_pages() {
        let tmp = tempfile::tempdir().unwrap();
        for n in 0..PAGE + 3 {
            fs::write(tmp.path().join(n.to_string()), b"").unwrap();
        }
        let mut picker = Picker::new(tmp.path(), false).unwrap();
        assert!(picker.more);
        let mut names = std::collections::BTreeSet::new();
        names.extend(
            picker
                .entries
                .iter()
                .filter(|e| !e.parent)
                .map(|e| e.name.clone()),
        );
        let last = picker.entries.last().unwrap().name.clone();
        fs::write(tmp.path().join("late arrival"), b"").unwrap();
        picker.page(true).unwrap();
        assert!(picker.entries[1].name > last);
        assert!(!picker.more);
        names.extend(
            picker
                .entries
                .iter()
                .filter(|e| !e.parent)
                .map(|e| e.name.clone()),
        );
        assert_eq!(names.len(), PAGE + 3);
        picker.page(false).unwrap();
        assert!(picker.more);
    }
    #[test]
    fn parent_row_is_navigation_only_and_location_paste_is_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        let mut p = Picker::new(&child, false).unwrap();
        assert!(p.entries[0].parent);
        p.cursor = 0;
        p.toggle().unwrap();
        assert!(p.selected.is_empty());
        p.edit_location();
        p.location_input.as_mut().unwrap().clear();
        p.paste_location("hello\n\x1b café");
        assert_eq!(p.location_input.as_deref(), Some("hello café"));
        p.paste_location(&"x".repeat(40000));
        assert!(p.location_input.as_ref().unwrap().len() <= 32768);
        p.location_input = None;
        let expected = p.directory.parent().unwrap().to_path_buf();
        p.open().unwrap();
        assert_eq!(p.directory, expected);
    }
    #[test]
    fn selection_survives_navigation_and_parent_selection_removes_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("folder")).unwrap();
        fs::write(tmp.path().join("folder/a"), b"a").unwrap();
        fs::write(tmp.path().join("b"), b"b").unwrap();
        let mut p = Picker::new(tmp.path(), false).unwrap();
        p.open().unwrap();
        p.toggle().unwrap();
        p.parent().unwrap();
        p.cursor = p.entries.iter().position(|e| e.name == "b").unwrap();
        p.toggle().unwrap();
        assert_eq!(p.sources().unwrap().len(), 2);
        p.cursor = p.entries.iter().position(|e| e.name == "folder").unwrap();
        p.toggle().unwrap();
        assert_eq!(p.sources().unwrap().len(), 2);
        assert!(p.selected.contains(&p.directory.join("folder")));
    }
    #[cfg(unix)]
    #[test]
    fn picker_never_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"a").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("a"), tmp.path().join("link")).unwrap();
        let p = Picker::new(tmp.path(), false).unwrap();
        assert_eq!(p.entries.iter().filter(|e| !e.parent).count(), 1);
    }
}
