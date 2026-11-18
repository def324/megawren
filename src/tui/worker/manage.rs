use super::*;
use std::sync::Arc;

const REVIEW_LIMIT: usize = 100_000;
fn reviewable(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || matches!(c,'\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'))
}
pub(super) struct Review {
    request: u64,
    details: Details,
    pub destructive: bool,
    waiting_stats: bool,
    // Bounded depth-first cursors; names and full directory trees are never retained.
    stack: Vec<(u64, usize, usize)>,
    files: usize,
    folders: usize,
}

fn valid_name(name: &str) -> Result<()> {
    state::component(name)?;
    ensure!(
        !name.is_empty() && name.len() <= 255,
        "Use a name of 1–255 bytes"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c)),
        "Name contains an unsupported character"
    );
    Ok(())
}

impl<R: Remote> Engine<'_, R> {
    pub(super) fn clear_review_many(&mut self) {
        self.review_queue.clear();
        self.review_members.clear();
        self.review_many = false;
    }
    pub(super) fn inspect_many(&mut self, request: u64, selections: Vec<Selection>) -> Result<()> {
        self.clear_review_many();
        self.idle()?;
        ensure!(
            !selections.is_empty() && selections.len() <= MAX_SELECTION,
            "Choose 1–{MAX_SELECTION} items"
        );
        let parent = selections[0].parent;
        let mut ids = std::collections::HashSet::new();
        ensure!(
            selections
                .iter()
                .all(|s| s.parent == parent && ids.insert(s.entry.id)),
            "Choose distinct items in one folder"
        );
        self.review_many = true;
        self.review_queue = selections.into();
        let first = self.review_queue.pop_front().unwrap();
        self.inspect(request, first, true)
    }
    fn idle(&self) -> Result<()> {
        ensure!(
            self.active.is_none() && self.queued.is_empty() && !self.changing,
            "Wait for transfers and file actions to finish"
        );
        Ok(())
    }
    fn selected(&mut self, selection: &Selection) -> Result<ffi::Metadata> {
        let meta = self.remote.metadata(selection.entry.id)?;
        let current = self.remote.info(selection.entry.id)?;
        ensure!(
            current.name == selection.entry.name
                && current.folder == selection.entry.folder
                && current.size == selection.entry.size
                && current.modified == selection.entry.modified
                && meta.parent == selection.parent
                && meta.generation == self.remote.generation(),
            "Item changed; refresh and select it again"
        );
        Ok(meta)
    }
    pub(super) fn inspect(
        &mut self,
        request: u64,
        selection: Selection,
        destructive: bool,
    ) -> Result<()> {
        self.reviewed = None;
        self.inspection = None;
        if destructive {
            self.idle()?;
            ensure!(
                self.remote.connection()? == 0,
                "Wait for MEGA to reconnect before reviewing removal"
            );
        }
        let meta = self.selected(&selection)?;
        if destructive {
            ensure!(
                !meta.restricted && !meta.in_rubbish && meta.access == 3,
                "Only private items owned by this account can be moved to the Rubbish Bin"
            );
            ensure!(
                !selection.entry.name.is_empty()
                    && reviewable(&selection.entry.name)
                    && reviewable(&meta.path),
                "This name cannot be reviewed safely in the terminal; use MEGA to manage it"
            );
        }
        let folder = selection.entry.folder;
        let id = selection.entry.id;
        let details = Details {
            members: self.review_members.clone(),
            selection,
            path: meta.path,
            created: meta.created,
            access: meta.access,
            in_rubbish: meta.in_rubbish,
            restore: (meta.restore != u64::MAX).then_some(meta.restore),
            generation: meta.generation,
            restricted: meta.restricted,
            favourite: meta.favourite,
            direct_link: meta.direct_link,
            stats: None,
            review_ready: false,
            checked: 0,
        };
        if folder {
            self.remote.start_stats(id)?;
        }
        update(&self.shared, |s| {
            s.details = Some(Inspection {
                request,
                details: Some(Arc::new(details.clone())),
                error: None,
            })
        });
        self.inspection = Some(Review {
            request,
            details,
            destructive,
            waiting_stats: folder,
            stack: if folder && destructive {
                vec![(id, 0, 0)]
            } else {
                vec![]
            },
            files: 0,
            folders: 0,
        });
        Ok(())
    }
    pub(super) fn inspection_error(&mut self, request: u64, error: anyhow::Error) {
        self.inspection = None;
        self.reviewed = None;
        self.clear_review_many();
        update(&self.shared, |s| {
            s.details = Some(Inspection {
                request,
                details: None,
                error: Some(safe(&error.to_string())),
            })
        });
    }
    fn review_step(&mut self, review: &mut Review) -> Result<bool> {
        ensure!(
            review.details.generation == self.remote.generation(),
            "Cloud changed during review; close this dialog and try again"
        );
        ensure!(
            self.review_members
                .iter()
                .all(|m| m.generation == review.details.generation),
            "Cloud changed during review; try again"
        );
        if review.waiting_stats {
            let stats = self.remote.stats()?;
            if stats.pending {
                return Ok(false);
            }
            ensure!(
                stats.ready && stats.code == 0,
                "Could not read folder contents ({})",
                stats.code
            );
            ensure!(
                [
                    stats.files,
                    stats.folders,
                    stats.bytes,
                    stats.versions,
                    stats.version_bytes
                ]
                .iter()
                .all(|n| *n >= 0),
                "Invalid folder information"
            );
            if review.destructive {
                ensure!(
                    stats.files.saturating_add(stats.folders) <= REVIEW_LIMIT as i64,
                    "Folder is too large to review safely in this app"
                );
            }
            review.details.stats = Some(FolderStats {
                files: stats.files,
                folders: stats.folders,
                bytes: stats.bytes,
                versions: stats.versions,
                version_bytes: stats.version_bytes,
            });
            review.waiting_stats = false;
        }
        if let Some((parent, offset, depth)) = review.stack.pop() {
            let rows = self.remote.list(parent, offset, 32)?;
            ensure!(rows.len() <= 32, "Invalid folder review page");
            if rows.len() == 32 {
                review.stack.push((parent, offset + 32, depth));
            }
            for row in rows {
                ensure!(
                    !row.shared && !row.exported,
                    "Folder contains shared or linked items; removal is blocked"
                );
                review.details.checked += 1;
                ensure!(
                    review.details.checked.saturating_add(
                        self.review_members.iter().map(|d| d.checked).sum::<usize>()
                    ) <= REVIEW_LIMIT,
                    "Folder is too large to review safely in this app"
                );
                if row.folder {
                    review.folders += 1;
                    ensure!(
                        depth < 256 && review.stack.len() < 8192,
                        "Folder structure is too complex to review safely"
                    );
                    review.stack.push((row.id, 0, depth + 1));
                } else {
                    review.files += 1;
                }
            }
            return Ok(false);
        }
        if review.destructive
            && let Some(stats) = &review.details.stats
        {
            ensure!(
                stats.files == review.files as i64 && stats.folders == review.folders as i64,
                "Folder contents changed; review again"
            );
        }
        review.details.review_ready = review.destructive;
        Ok(true)
    }
    pub(super) fn management_tick(&mut self) {
        if self
            .reviewed
            .as_ref()
            .is_some_and(|r| r.generation != self.remote.generation())
        {
            let request = snapshot(&self.shared).details.as_ref().map(|d| d.request);
            if let Some(request) = request {
                self.inspection_error(
                    request,
                    anyhow::anyhow!("Cloud changed; close this dialog and review again"),
                );
            }
        }
        if self.changing {
            match self.remote.event() {
                Ok(event) if !event.done => (),
                result => {
                    self.changing = false;
                    update(&self.shared, |s| s.action = None);
                    match result {
                        Ok(e) if e.code == 0 => self.notice("File action completed".into()),
                        Ok(e) => {
                            let detail = self.remote.errors();
                            self.notice(format!(
                                "{} of {} completed · {}",
                                e.bytes,
                                e.total.max(1),
                                if detail.is_empty() {
                                    transfer_error(e.code)
                                } else {
                                    detail
                                }
                            ));
                        }
                        Err(e) => {
                            if let Err(error) = self.quiesce(false) {
                                self.closing = true;
                                self.fatal = Some(error.to_string());
                                return;
                            }
                            self.notice(e.to_string());
                        }
                    }
                    if let Some((r, p, o)) = self.current {
                        self.page(r, p, o);
                    }
                    let _ = self.remote.refresh_account();
                }
            }
        }
        if let Some(mut review) = self.inspection.take() {
            match self.review_step(&mut review) {
                Err(e) => self.inspection_error(review.request, e),
                Ok(done) => {
                    if done && review.destructive && self.review_many {
                        review.details.members.clear();
                        self.review_members.push(Arc::new(review.details));
                        if let Some(next) = self.review_queue.pop_front() {
                            if let Err(e) = self.inspect(review.request, next, true) {
                                self.inspection_error(review.request, e);
                            }
                        } else {
                            let mut aggregate = (*self.review_members[0]).clone();
                            aggregate.members = std::mem::take(&mut self.review_members);
                            aggregate.checked = aggregate.members.iter().map(|d| d.checked).sum();
                            let aggregate = Arc::new(aggregate);
                            self.reviewed = Some(aggregate.clone());
                            self.review_many = false;
                            update(&self.shared, |s| {
                                s.details = Some(Inspection {
                                    request: review.request,
                                    details: Some(aggregate),
                                    error: None,
                                })
                            });
                        }
                        return;
                    }
                    let details = Arc::new(review.details.clone());
                    update(&self.shared, |s| {
                        s.details = Some(Inspection {
                            request: review.request,
                            details: Some(details.clone()),
                            error: None,
                        })
                    });
                    if done {
                        if review.destructive {
                            self.reviewed = Some(details);
                        }
                    } else {
                        self.inspection = Some(review);
                    }
                }
            }
        }
    }
    pub(super) fn change(&mut self, change: Change) -> Result<()> {
        if let Change::Many { selections, action } = change {
            return self.change_many(selections, action);
        }
        if let Change::Trash { ref review } = change
            && !review.members.is_empty()
        {
            return self.trash_many(review.clone());
        }
        self.idle()?;
        ensure!(
            self.remote.connection()? == 0,
            "Wait for MEGA to reconnect before changing files"
        );
        let (kind, source, parent, name, epoch, label) = match change {
            Change::Many { .. } => unreachable!(),
            Change::NewFolder { parent, name } => {
                valid_name(&name)?;
                (
                    5,
                    parent,
                    parent,
                    name,
                    self.remote.generation(),
                    "Creating folder…",
                )
            }
            Change::Trash { review } => {
                ensure!(
                    self.reviewed
                        .as_ref()
                        .is_some_and(|r| Arc::ptr_eq(r, &review))
                        && review.review_ready,
                    "Review this item before moving it to the Rubbish Bin"
                );
                let m = self.selected(&review.selection)?;
                ensure!(
                    !m.restricted && !m.in_rubbish && m.generation == review.generation,
                    "Cloud changed; review this item again"
                );
                (
                    4,
                    review.selection.entry.id,
                    0,
                    String::new(),
                    review.generation,
                    "Moving to Rubbish Bin…",
                )
            }
            Change::Rename { selection, name } => {
                valid_name(&name)?;
                let m = self.selected(&selection)?;
                ensure!(
                    !m.restricted && !m.in_rubbish,
                    "This item cannot be renamed"
                );
                (
                    1,
                    selection.entry.id,
                    selection.parent,
                    name,
                    m.generation,
                    "Renaming…",
                )
            }
            Change::Copy {
                selection,
                destination,
                name,
            } => {
                valid_name(&name)?;
                let m = self.selected(&selection)?;
                ensure!(!m.restricted && !m.in_rubbish, "This item cannot be copied");
                (
                    3,
                    selection.entry.id,
                    destination,
                    name,
                    m.generation,
                    "Copying…",
                )
            }
        };
        if kind != 4 {
            let found = self.remote.find(parent, &name)?;
            ensure!(
                found.is_none(),
                "An item with that name already exists; choose another name or folder"
            );
        }
        self.remote.change(kind, source, parent, &name, epoch)?;
        self.inspection = None;
        self.reviewed = None;
        self.changing = true;
        update(&self.shared, |s| {
            s.action = Some(label.into());
            s.details = None;
        });
        Ok(())
    }
    fn submit_many(
        &mut self,
        kind: u8,
        sources: Vec<u64>,
        parents: Vec<u64>,
        names: Vec<String>,
        epoch: u64,
    ) -> Result<()> {
        self.remote
            .change_many(kind, &sources, &parents, &names, epoch)?;
        self.inspection = None;
        self.reviewed = None;
        self.clear_review_many();
        self.changing = true;
        update(&self.shared, |s| {
            s.action = Some(format!(
                "{} {} items…",
                match kind {
                    2 => "Moving",
                    3 => "Copying",
                    4 => "Moving to Rubbish Bin:",
                    _ => "Restoring",
                },
                sources.len()
            ));
            s.details = None;
        });
        Ok(())
    }
    fn trash_many(&mut self, review: Arc<Details>) -> Result<()> {
        self.idle()?;
        ensure!(self.remote.connection()? == 0, "Wait for MEGA to reconnect");
        ensure!(
            self.reviewed
                .as_ref()
                .is_some_and(|r| Arc::ptr_eq(r, &review))
                && review.review_ready,
            "Review this selection again"
        );
        let mut sources = vec![];
        for member in &review.members {
            let meta = self.selected(&member.selection)?;
            ensure!(
                !meta.restricted && !meta.in_rubbish && meta.generation == review.generation,
                "Cloud changed; review this selection again"
            );
            sources.push(member.selection.entry.id);
        }
        ensure!(
            !sources.is_empty() && sources.len() <= MAX_SELECTION,
            "Invalid selection"
        );
        self.submit_many(
            4,
            sources.clone(),
            vec![0; sources.len()],
            vec![String::new(); sources.len()],
            review.generation,
        )
    }
    fn change_many(&mut self, selections: Vec<Selection>, action: BatchAction) -> Result<()> {
        self.idle()?;
        ensure!(self.remote.connection()? == 0, "Wait for MEGA to reconnect");
        ensure!(
            !selections.is_empty() && selections.len() <= MAX_SELECTION,
            "Choose 1–{MAX_SELECTION} items"
        );
        let epoch = self.remote.generation();
        let parent = selections[0].parent;
        let mut ids = std::collections::HashSet::new();
        let mut names_set = std::collections::HashSet::new();
        let (mut sources, mut parents, mut names) = (vec![], vec![], vec![]);
        let kind = match action {
            BatchAction::Move(_) => 2,
            BatchAction::Copy(_) => 3,
            BatchAction::Restore => 6,
        };
        for selection in selections {
            ensure!(
                selection.parent == parent && ids.insert(selection.entry.id),
                "Choose distinct items in one folder"
            );
            let meta = self.selected(&selection)?;
            ensure!(
                !meta.restricted && meta.generation == epoch,
                "Selection changed or includes protected items"
            );
            let destination = match action {
                BatchAction::Move(id) | BatchAction::Copy(id) => {
                    ensure!(!meta.in_rubbish, "Restore items before changing them");
                    id
                }
                BatchAction::Restore => {
                    ensure!(
                        meta.in_rubbish && meta.restore != u64::MAX,
                        "Original folder unavailable"
                    );
                    meta.restore
                }
            };
            valid_name(&selection.entry.name)?;
            ensure!(
                names_set.insert((destination, selection.entry.name.clone()))
                    && self
                        .remote
                        .find(destination, &selection.entry.name)?
                        .is_none(),
                "Destination has a conflicting name; choose another folder"
            );
            sources.push(selection.entry.id);
            parents.push(destination);
            names.push(selection.entry.name);
        }
        self.submit_many(kind, sources, parents, names, epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::reviewable;
    #[test]
    fn removal_review_rejects_terminal_controls_and_invisible_direction_changes() {
        for name in ["", "/Photos", "/café/写真"] {
            assert!(reviewable(name));
        }
        for name in [
            "bad\x1b[2J",
            "line\nname",
            "photo\u{202e}gpj",
            "invisible\u{200b}",
            "\u{2066}file",
        ] {
            assert!(!reviewable(name));
        }
    }
}
