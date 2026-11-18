use super::*;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum NameKind {
    Rename,
    NewFolder,
    Copy,
}
pub(super) enum Dialog {
    Menu {
        request: Option<u64>,
        targets: Vec<Selection>,
        cursor: usize,
    },
    TrashReview {
        request: u64,
        confirm: bool,
        scroll: u16,
        targets: Vec<Selection>,
    },
    Name {
        kind: NameKind,
        selection: Option<Selection>,
        parent: u64,
        location: String,
        value: String,
        error: Option<String>,
    },
    Destination {
        selections: Vec<Selection>,
        copy: bool,
        button: bool,
        folders: Vec<(u64, String)>,
        request: u64,
        offset: usize,
        cursor: usize,
    },
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ItemAction {
    Download,
    Preview,
    Rename,
    Move,
    Copy,
    Trash,
    Restore,
}

impl ItemAction {
    pub(super) fn binding(self) -> (&'static str, Option<KeyCode>) {
        match self {
            Self::Download => ("Download…", Some(KeyCode::Char('d'))),
            Self::Preview => ("Preview", Some(KeyCode::Char('p'))),
            Self::Rename => ("Rename", Some(KeyCode::F(2))),
            Self::Move => ("Move…", Some(KeyCode::Char('m'))),
            Self::Copy => ("Copy…", Some(KeyCode::Char('c'))),
            Self::Trash => ("Move to Rubbish Bin", Some(KeyCode::Delete)),
            Self::Restore => ("Restore to original folder", None),
        }
    }
}

pub(super) fn item_actions(targets: &[Selection], in_bin: bool) -> Vec<ItemAction> {
    use ItemAction::*;
    if targets.is_empty() {
        return Vec::new();
    }
    let mut actions = vec![Download];
    if let [item] = targets
        && crate::preview::plan(&item.entry.name, item.entry.size, item.entry.folder).is_ok()
    {
        actions.push(Preview);
    }
    if in_bin {
        actions.push(Restore);
    } else {
        if targets.len() == 1 {
            actions.push(Rename);
        }
        actions.extend([Move, Copy, Trash]);
    }
    actions
}

fn text_key(value: &mut String, key: KeyEvent, limit: usize) {
    match key.code {
        KeyCode::Backspace => {
            value.pop();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => value.clear(),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && !c.is_control()
                && value.len() + c.len_utf8() <= limit =>
        {
            value.push(c)
        }
        _ => (),
    }
}
impl Ui {
    fn selection(&self) -> Option<Selection> {
        self.page.get(self.cursor).cloned().map(|entry| Selection {
            entry,
            parent: self.parent(),
        })
    }
    pub(super) fn serial(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }
    pub(super) fn targets(&self) -> Vec<Selection> {
        if self.marked.is_empty() {
            self.selection().into_iter().collect()
        } else {
            self.marked.clone()
        }
    }
    pub(super) fn toggle_mark(&mut self) {
        let Some(selection) = self.selection() else {
            return;
        };
        if let Some(i) = self
            .marked
            .iter()
            .position(|s| s.entry.id == selection.entry.id)
        {
            self.marked.remove(i);
        } else if self.marked.len() < MAX_SELECTION {
            self.marked.push(selection);
        } else {
            self.notice = Some((
                format!("Choose up to {MAX_SELECTION} items"),
                Instant::now(),
            ));
        }
    }
    pub(super) fn open_sheet(&mut self, commands: &SyncSender<Command>) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let request = if let [target] = targets.as_slice() {
            let id = self.serial();
            self.send(
                commands,
                Command::Inspect {
                    request: id,
                    selection: target.clone(),
                    review: false,
                },
            )
            .then_some(id)
        } else {
            None
        };
        self.modal = Some(Modal::Action(Dialog::Menu {
            cursor: 0,
            targets,
            request,
        }));
    }
    pub(super) fn review_trash(&mut self, commands: &SyncSender<Command>) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let request = self.serial();
        if self.send(
            commands,
            Command::InspectMany {
                request,
                selections: targets.clone(),
            },
        ) {
            self.modal = Some(Modal::Action(Dialog::TrashReview {
                request,
                confirm: false,
                scroll: 0,
                targets,
            }));
        }
    }
    pub(super) fn edit_name(&mut self, kind: NameKind) {
        let targets = self.targets();
        if kind == NameKind::Rename && targets.len() > 1 {
            self.notice = Some(("Rename one item at a time".into(), Instant::now()));
            return;
        }
        let selection = targets.first().cloned();
        if kind != NameKind::NewFolder && selection.is_none() {
            return;
        }
        let value = if kind == NameKind::Rename {
            selection.as_ref().unwrap().entry.name.clone()
        } else {
            String::new()
        };
        self.modal = Some(Modal::Action(Dialog::Name {
            kind,
            selection,
            parent: self.parent(),
            location: self.path(),
            value,
            error: None,
        }));
    }
    pub(super) fn destination(&mut self, copy: bool, commands: &SyncSender<Command>) {
        let selections = self.targets();
        if !selections.is_empty() {
            let request = self.serial();
            if self.send(
                commands,
                Command::PickFolder {
                    request,
                    parent: self.root,
                    offset: 0,
                },
            ) {
                self.modal = Some(Modal::Action(Dialog::Destination {
                    selections,
                    copy,
                    button: false,
                    folders: vec![(self.root, String::new())],
                    request,
                    offset: 0,
                    cursor: 0,
                }));
            }
        }
    }
    pub(super) fn switch_location(
        &mut self,
        bin: bool,
        state: &Snapshot,
        commands: &SyncSender<Command>,
    ) {
        if bin == self.in_bin() {
            return;
        }
        let root = if bin {
            let Some(root) = state.rubbish else {
                self.notice = Some(("Rubbish Bin is not available yet".into(), Instant::now()));
                return;
            };
            root
        } else {
            self.root
        };
        let (parent, offset) = self
            .other_location
            .as_ref()
            .map(|location| (location.folders.last().unwrap().0, location.offset))
            .unwrap_or((root, 0));
        let request = self.request.wrapping_add(1);
        if !self.send(
            commands,
            Command::List {
                request,
                parent,
                offset,
            },
        ) {
            return;
        }
        let next = self.other_location.take().unwrap_or(BrowserLocation {
            folders: vec![(root, String::new())],
            offset: 0,
            cursor: 0,
            viewport: 0,
            page: Arc::new(Vec::new()),
        });
        self.other_location = Some(BrowserLocation {
            folders: std::mem::replace(&mut self.folders, next.folders),
            offset: self.offset,
            cursor: self.cursor,
            viewport: self.viewport.get(),
            page: std::mem::replace(&mut self.page, next.page),
        });
        self.offset = next.offset;
        self.cursor = next.cursor;
        self.viewport.set(next.viewport);
        self.request = request;
        self.loading = true;
        self.notice = None;
        self.marked.clear();
    }
    pub(super) fn paste(&mut self, text: &str) {
        match self.modal.as_mut() {
            Some(Modal::Upload { picker, .. } | Modal::Download { picker, .. }) => {
                picker.paste_location(text)
            }
            Some(Modal::Action(Dialog::Name { value, .. })) => append(value, text, 255),
            _ => (),
        }
    }
    pub(super) fn action_key(
        &mut self,
        mut dialog: Dialog,
        key: KeyEvent,
        state: &Snapshot,
        commands: &SyncSender<Command>,
    ) -> Option<Dialog> {
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('i') && matches!(dialog, Dialog::Menu { .. }))
        {
            if let Dialog::TrashReview { request, .. }
            | Dialog::Menu {
                request: Some(request),
                ..
            } = &dialog
            {
                self.send(commands, Command::CancelInspect(*request));
            }
            return None;
        }
        match &mut dialog {
            Dialog::Menu {
                cursor,
                targets,
                request,
            } => {
                if !self
                    .targets()
                    .iter()
                    .map(|s| (s.entry.id, &s.entry.name, s.parent))
                    .eq(targets
                        .iter()
                        .map(|s| (s.entry.id, &s.entry.name, s.parent)))
                {
                    if let Some(id) = *request {
                        self.send(commands, Command::CancelInspect(id));
                    }
                    self.notice =
                        Some(("Selection changed. Open it again.".into(), Instant::now()));
                    return None;
                }
                let choices = item_actions(targets, self.in_bin());
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => *cursor = cursor.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        *cursor = (*cursor + 1).min(choices.len().saturating_sub(1))
                    }
                    code => {
                        let action = if code == KeyCode::Enter {
                            choices.get(*cursor)
                        } else {
                            choices
                                .iter()
                                .find(|action| action.binding().1 == Some(code))
                        };
                        let Some(action) = action else {
                            return Some(dialog);
                        };
                        if let Some(id) = *request {
                            self.send(commands, Command::CancelInspect(id));
                        }
                        match action {
                            ItemAction::Rename => self.edit_name(NameKind::Rename),
                            ItemAction::Move => self.destination(false, commands),
                            ItemAction::Copy => self.destination(true, commands),
                            ItemAction::Trash => self.review_trash(commands),
                            ItemAction::Restore => {
                                if self.send(
                                    commands,
                                    Command::Change(Change::Many {
                                        selections: targets.clone(),
                                        action: BatchAction::Restore,
                                    }),
                                ) {
                                    self.marked.clear();
                                }
                            }
                            ItemAction::Download => self.open_download(),
                            ItemAction::Preview => {
                                self.preview_entry(targets[0].entry.clone(), commands)
                            }
                        }
                        return None;
                    }
                }
            }
            Dialog::Name {
                kind,
                selection,
                parent,
                value,
                error,
                ..
            } => {
                if key.code == KeyCode::Enter {
                    if *kind == NameKind::Rename
                        && selection.as_ref().is_some_and(|s| s.entry.name == *value)
                    {
                        return None;
                    }
                    if value.is_empty() {
                        *error = Some("Enter a name".into());
                    } else {
                        let change = match kind {
                            NameKind::NewFolder => Change::NewFolder {
                                parent: *parent,
                                name: value.clone(),
                            },
                            NameKind::Rename => Change::Rename {
                                selection: selection.clone().unwrap(),
                                name: value.clone(),
                            },
                            NameKind::Copy => Change::Copy {
                                selection: selection.clone().unwrap(),
                                destination: *parent,
                                name: value.clone(),
                            },
                        };
                        if self.send(commands, Command::Change(change)) {
                            self.marked.clear();
                            return None;
                        }
                    }
                } else {
                    text_key(value, key, 255);
                    *error = None;
                }
            }
            Dialog::TrashReview {
                request,
                confirm,
                scroll,
                ..
            } => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        *scroll = scroll.saturating_add(1).min(self.detail_scroll_limit.get())
                    }
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    KeyCode::PageDown => {
                        *scroll = scroll
                            .saturating_add(10)
                            .min(self.detail_scroll_limit.get())
                    }
                    _ => (),
                }
                if let Some(inspection) = state.details.as_ref().filter(|d| d.request == *request)
                    && let Some(details) = &inspection.details
                    && details.review_ready
                    && self.review_visible.get()
                {
                    match key.code {
                        KeyCode::Left => *confirm = false,
                        KeyCode::Right => *confirm = true,
                        KeyCode::Tab | KeyCode::BackTab => *confirm = !*confirm,
                        KeyCode::Enter if *confirm => {
                            if self.send(
                                commands,
                                Command::Change(Change::Trash {
                                    review: details.clone(),
                                }),
                            ) {
                                self.marked.clear();
                                return None;
                            }
                        }
                        KeyCode::Enter => {
                            self.send(commands, Command::CancelInspect(*request));
                            return None;
                        }
                        _ => (),
                    }
                }
            }
            Dialog::Destination {
                selections,
                copy,
                button,
                folders,
                request,
                offset,
                cursor,
            } => {
                let page = state.picker_page.as_ref().filter(|p| {
                    p.request == *request
                        && p.parent == folders.last().unwrap().0
                        && p.offset == *offset
                });
                let count = page.map_or(0, |p| p.entries.len());
                let shortcut = if *copy { 'c' } else { 'm' };
                let submit = (key.code == KeyCode::Enter && *button)
                    || (key.modifiers.is_empty() && key.code == KeyCode::Char(shortcut));
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *button = false;
                        *cursor = cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *button = false;
                        *cursor = (*cursor + 1).min(count);
                    }
                    KeyCode::Tab | KeyCode::BackTab => *button = !*button,
                    _ if submit => {
                        if !page.is_some_and(|p| p.error.is_none()) {
                            return Some(dialog);
                        }
                        let parent = folders.last().unwrap().0;
                        if *copy && selections.len() == 1 {
                            let selection = &selections[0];
                            return Some(Dialog::Name {
                                kind: NameKind::Copy,
                                selection: Some(selection.clone()),
                                parent,
                                location: format!(
                                    "/{}",
                                    folders
                                        .iter()
                                        .skip(1)
                                        .map(|(_, n)| n.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/")
                                ),
                                value: selection.entry.name.clone(),
                                error: None,
                            });
                        }
                        if self.send(
                            commands,
                            Command::Change(Change::Many {
                                selections: selections.clone(),
                                action: if *copy {
                                    BatchAction::Copy(parent)
                                } else {
                                    BatchAction::Move(parent)
                                },
                            }),
                        ) {
                            self.marked.clear();
                            return None;
                        }
                    }
                    KeyCode::Enter
                    | KeyCode::Right
                    | KeyCode::Char('l')
                    | KeyCode::Left
                    | KeyCode::Backspace
                    | KeyCode::Char('h')
                    | KeyCode::PageDown
                    | KeyCode::PageUp => {
                        let current = folders.last().unwrap().0;
                        let mut child = None;
                        let (parent, next_offset) = match key.code {
                            KeyCode::PageDown if page.is_some_and(|p| p.more) => {
                                (current, *offset + PAGE_SIZE)
                            }
                            KeyCode::PageUp if *offset > 0 => {
                                (current, offset.saturating_sub(PAGE_SIZE))
                            }
                            KeyCode::PageUp | KeyCode::PageDown => return Some(dialog),
                            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if *cursor > 0 => {
                                let Some(row) = page.and_then(|p| p.entries.get(*cursor - 1))
                                else {
                                    return Some(dialog);
                                };
                                child = Some((row.id, row.name.clone()));
                                (row.id, 0)
                            }
                            _ if folders.len() > 1 => (folders[folders.len() - 2].0, 0),
                            _ => return Some(dialog),
                        };
                        let next = self.serial();
                        if self.send(
                            commands,
                            Command::PickFolder {
                                request: next,
                                parent,
                                offset: next_offset,
                            },
                        ) {
                            if let Some(child) = child {
                                folders.push(child);
                            } else if current != parent {
                                folders.pop();
                            }
                            *request = next;
                            *cursor = 0;
                            *offset = next_offset;
                            *button = false;
                        }
                    }
                    _ => (),
                }
            }
        }
        Some(dialog)
    }
}
fn append(value: &mut String, text: &str, limit: usize) {
    for c in text.chars() {
        if !c.is_control() && value.len() + c.len_utf8() <= limit {
            value.push(c);
        }
    }
}
