use super::*;
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn fixture() -> (Ui, Snapshot) {
    let mut ui = Ui::new(1).unwrap();
    ui.loading = false;
    ui.page = Arc::new(vec![Entry {
        id: 3,
        name: "folder".into(),
        size: 0,
        folder: true,
        modified: 0,
        shared: false,
        exported: false,
        parent: 0,
        fingerprint: String::new(),
    }]);
    (
        ui,
        Snapshot {
            rubbish: Some(9),
            ..Snapshot::default()
        },
    )
}
fn ready(ui: &Ui, request: u64) -> Inspection {
    Inspection {
        request,
        details: Some(Arc::new(Details {
            members: Vec::new(),
            selection: ui.selection().unwrap(),
            path: "/folder".into(),
            created: 1,
            access: 3,
            in_rubbish: false,
            restore: None,
            generation: 1,
            restricted: false,
            favourite: false,
            direct_link: crate::bridge::ffi::LinkState::None,
            stats: Some(FolderStats {
                files: 3,
                folders: 2,
                bytes: 42,
                versions: 0,
                version_bytes: 0,
            }),
            review_ready: true,
            checked: 5,
        })),
        error: None,
    }
}
fn draw(ui: &Ui, state: &Snapshot, width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|frame| view::draw(frame, ui, state)).unwrap();
    term.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
#[test]
fn item_info_and_icon_style_have_separate_keys() {
    let (mut ui, state) = fixture();
    let (send, recv) = sync_channel(16);
    let cancel = AtomicBool::new(false);
    for code in [KeyCode::Char('a'), KeyCode::Tab, KeyCode::Char('b')] {
        ui.key(key(code), &state, &send, &cancel);
        assert!(ui.modal.is_none() && !ui.in_bin());
    }
    ui.key(key(KeyCode::Char('i')), &state, &send, &cancel);
    assert!(matches!(ui.modal, Some(Modal::Action(Dialog::Menu { .. }))));
    assert!(matches!(
        recv.try_recv(),
        Ok(Command::Inspect { review: false, .. })
    ));
    assert_eq!(ui.parent(), 1);
    ui.key(key(KeyCode::Char('i')), &state, &send, &cancel);
    assert!(ui.modal.is_none());
    assert!(matches!(recv.try_recv(), Ok(Command::CancelInspect(_))));
    ui.key(key(KeyCode::Char('?')), &state, &send, &cancel);
    for code in [KeyCode::Char('i'), KeyCode::F(3)] {
        ui.key(key(code), &state, &send, &cancel);
        assert!(matches!(ui.modal, Some(Modal::Help)) && ui.nerd_icons);
    }
    ui.key(key(KeyCode::Char('?')), &state, &send, &cancel);
    assert!(ui.modal.is_none());
    ui.key(key(KeyCode::F(3)), &state, &send, &cancel);
    assert!(!ui.nerd_icons);
    ui.key(key(KeyCode::Char('u')), &state, &send, &cancel);
    ui.key(key(KeyCode::Char('i')), &state, &send, &cancel);
    assert!(!ui.nerd_icons && matches!(ui.modal, Some(Modal::Upload { .. })));
    ui.key(key(KeyCode::F(3)), &state, &send, &cancel);
    assert!(ui.nerd_icons);
}

#[test]
fn modified_shortcuts_never_submit_an_item_action() {
    let (mut ui, state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    for code in [KeyCode::Char('u'), KeyCode::Delete, KeyCode::Char('q')] {
        ui.key(
            KeyEvent::new(code, KeyModifiers::ALT),
            &state,
            &send,
            &cancel,
        );
        assert!(ui.modal.is_none() && !ui.closed);
    }
    assert!(recv.try_recv().is_err());
}

#[test]
fn removal_uses_one_confirmation_with_cancel_selected_and_no_typing() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(16);
    let cancel = AtomicBool::new(false);
    ui.key(key(KeyCode::Delete), &state, &send, &cancel);
    let Command::InspectMany { request, .. } = recv.try_recv().unwrap() else {
        panic!("Review required");
    };
    draw(&ui, &state, 100, 28);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    state.details = Some(ready(&ui, request));
    let screen = draw(&ui, &state, 100, 28);
    assert!(screen.contains("[ Cancel ]"));
    assert!(!screen.contains("Type the"));
    ui.key(key(KeyCode::Left), &state, &send, &cancel);
    assert!(draw(&ui, &state, 100, 28).contains("[ Cancel ]"));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(matches!(
        recv.try_recv().unwrap(),
        Command::CancelInspect(_)
    ));
    assert!(ui.modal.is_none());
    ui.key(key(KeyCode::Delete), &state, &send, &cancel);
    let Command::InspectMany { request, .. } = recv.try_recv().unwrap() else {
        panic!()
    };
    state.details = Some(ready(&ui, request));
    draw(&ui, &state, 100, 28);
    ui.key(key(KeyCode::Right), &state, &send, &cancel);
    ui.key(key(KeyCode::Right), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    let Command::Change(Change::Trash { review }) = recv.try_recv().unwrap() else {
        panic!()
    };
    assert!(Arc::ptr_eq(
        &review,
        state.details.unwrap().details.as_ref().unwrap()
    ));
    assert!(ui.modal.is_none());
}

#[test]
fn removal_stays_blocked_when_terminal_is_small_or_review_is_invalidated() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    ui.review_trash(&send);
    let Command::InspectMany { request, .. } = recv.try_recv().unwrap() else {
        panic!()
    };
    state.details = Some(ready(&ui, request));
    assert!(draw(&ui, &state, 60, 20).contains("Enlarge the terminal"));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(ui.modal.is_some());
    draw(&ui, &state, 100, 28);
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    state.details = Some(Inspection {
        request,
        details: None,
        error: Some("Cloud changed; review again".into()),
    });
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(draw(&ui, &state, 100, 28).contains("Cloud changed"));
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    assert!(matches!(
        recv.try_recv().unwrap(),
        Command::CancelInspect(_)
    ));
}

#[test]
fn destination_shortcut_uses_the_displayed_folder_only_when_ready() {
    for (copy, shortcut) in [(true, 'c'), (false, 'm')] {
        let (mut ui, mut state) = fixture();
        let (send, recv) = sync_channel(8);
        let cancel = AtomicBool::new(false);
        ui.destination(copy, &send);
        let Command::PickFolder {
            request, parent, ..
        } = recv.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(draw(&ui, &state, 100, 28).contains(if copy {
            "c  Copy here"
        } else {
            "m  Move here"
        }));
        ui.key(key(KeyCode::Char(shortcut)), &state, &send, &cancel);
        assert!(matches!(
            ui.modal,
            Some(Modal::Action(Dialog::Destination { .. }))
        ));
        assert!(recv.try_recv().is_err());
        state.picker_page = Some(Page {
            more: false,
            request,
            parent,
            offset: 0,
            entries: Arc::new(vec![]),
            error: None,
        });
        ui.key(
            key(KeyCode::Char(if copy { 'm' } else { 'c' })),
            &state,
            &send,
            &cancel,
        );
        assert!(matches!(
            ui.modal,
            Some(Modal::Action(Dialog::Destination { .. }))
        ));
        ui.key(key(KeyCode::Char(shortcut)), &state, &send, &cancel);
        if copy {
            assert!(matches!(
                ui.modal,
                Some(Modal::Action(Dialog::Name {
                    kind: NameKind::Copy,
                    ..
                }))
            ));
            assert!(recv.try_recv().is_err());
        } else {
            assert!(
                matches!(recv.try_recv().unwrap(), Command::Change(Change::Many { action: BatchAction::Move(id), .. }) if id == parent)
            );
        }
    }
}
#[test]
fn copy_requires_current_destination_response_and_explicit_name_submission() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    ui.destination(true, &send);
    let Command::PickFolder { request, .. } = recv.try_recv().unwrap() else {
        panic!()
    };
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    let mut folder = ui.page[0].clone();
    folder.id = 8;
    folder.name = "Backups".into();
    state.picker_page = Some(Page {
        more: false,
        request,
        parent: 1,
        offset: 0,
        entries: Arc::new(vec![folder]),
        error: None,
    });
    ui.key(key(KeyCode::BackTab), &state, &send, &cancel);
    assert!(matches!(
        ui.modal,
        Some(Modal::Action(Dialog::Destination { button: true, .. }))
    ));
    ui.key(key(KeyCode::Down), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    let Command::PickFolder {
        request, parent: 8, ..
    } = recv.try_recv().unwrap()
    else {
        panic!()
    };
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    assert!(matches!(
        ui.modal,
        Some(Modal::Action(Dialog::Destination { .. }))
    ));
    state.picker_page = Some(Page {
        more: false,
        request,
        parent: 8,
        offset: 0,
        entries: Arc::new(vec![]),
        error: None,
    });
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(draw(&ui, &state, 100, 28).contains("In: /Backups"));
    ui.key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &state,
        &send,
        &cancel,
    );
    ui.paste("copy of folder");
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(
        matches!(recv.try_recv().unwrap(),Command::Change(Change::Copy{destination:8,name,..}) if name=="copy of folder")
    );
}
#[test]
fn rubbish_bin_item_menu_keeps_location_switching_in_the_browser() {
    let (mut ui, state) = fixture();
    let (send, _recv) = sync_channel(8);
    ui.switch_location(true, &state, &send);
    ui.loading = false;
    ui.page = fixture().0.page;
    ui.modal = Some(Modal::Action(Dialog::Menu {
        cursor: 0,
        request: None,
        targets: ui.targets(),
    }));
    let screen = draw(&ui, &state, 100, 28);
    assert!(screen.contains("Restore to original folder"));
    assert!(!screen.contains("Move to Rubbish Bin"));
    assert!(!screen.contains("Rename"));
    assert!(!screen.contains("Back to Cloud drive"));
}

#[test]
fn bin_has_visible_entry_and_exit_even_when_empty_loading_or_compact() {
    for exit in [
        KeyCode::Left,
        KeyCode::Backspace,
        KeyCode::Char('h'),
        KeyCode::Esc,
        KeyCode::Char('1'),
    ] {
        let (mut ui, mut state) = fixture();
        let (send, recv) = sync_channel(8);
        let cancel = AtomicBool::new(false);
        ui.folders.push((4, "Photos".into()));
        ui.offset = PAGE_SIZE;
        ui.cursor = 5;
        ui.viewport.set(3);
        assert!(draw(&ui, &state, 38, 14).contains("2 Rubbish Bin"));
        let previous_viewport = ui.viewport.get();
        ui.key(key(KeyCode::Char('2')), &state, &send, &cancel);
        assert!(matches!(
            recv.try_recv().unwrap(),
            Command::List {
                parent: 9,
                offset: 0,
                ..
            }
        ));
        assert!(ui.in_bin());
        // Returning must work before the bin's page has arrived, including on a stale snapshot.
        state.rubbish = None;
        ui.key(key(exit), &state, &send, &cancel);
        assert!(!ui.in_bin());
        assert_eq!(ui.path(), "/Photos");
        assert_eq!(
            (ui.offset, ui.cursor, ui.viewport.get()),
            (PAGE_SIZE, 5, previous_viewport)
        );
        assert!(matches!(
            recv.try_recv().unwrap(),
            Command::List {
                parent: 4,
                offset: PAGE_SIZE,
                ..
            }
        ));
        state.rubbish = Some(9);
        ui.key(key(KeyCode::Char('2')), &state, &send, &cancel);
        let _ = recv.try_recv().unwrap();
        ui.loading = false;
        let screen = draw(&ui, &state, 70, 24);
        assert!(screen.contains("Rubbish Bin is empty"));
        assert!(screen.contains("← Cloud drive"));
        assert!(!screen.contains("Press u to upload"));
        assert!(draw(&ui, &state, 38, 14).contains("Esc return"));
    }
}

#[test]
fn switching_preserves_both_locations_and_parent_and_modal_navigation_remain_local() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    ui.key(key(KeyCode::Char('2')), &state, &send, &cancel);
    let _ = recv.try_recv().unwrap();
    ui.folders.push((8, "Deleted folder".into()));
    ui.loading = false;
    ui.key(key(KeyCode::Left), &state, &send, &cancel);
    assert!(ui.in_bin());
    assert_eq!(ui.parent(), 9);
    let _ = recv.try_recv().unwrap();
    ui.folders.push((8, "Deleted folder".into()));
    ui.offset = PAGE_SIZE;
    ui.cursor = 2;
    ui.key(key(KeyCode::Char('1')), &state, &send, &cancel);
    let _ = recv.try_recv().unwrap();
    ui.key(key(KeyCode::Char('2')), &state, &send, &cancel);
    assert!(matches!(
        recv.try_recv().unwrap(),
        Command::List {
            parent: 8,
            offset: PAGE_SIZE,
            ..
        }
    ));
    assert_eq!(ui.cursor, 2);
    ui.modal = Some(Modal::Help);
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    assert!(ui.modal.is_none());
    assert!(ui.in_bin());
    ui.modal = Some(Modal::Action(Dialog::Menu {
        cursor: 0,
        request: None,
        targets: ui.targets(),
    }));
    ui.key(key(KeyCode::Char('1')), &state, &send, &cancel);
    assert!(ui.modal.is_some() && ui.in_bin());
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    ui.key(key(KeyCode::Char('1')), &state, &send, &cancel);
    assert!(!ui.in_bin());
    let selected = ui.page[ui.cursor].clone();
    let mut other = selected.clone();
    other.id += 10;
    other.name = "New file".into();
    state.revision = 1;
    state.page = Some(Page {
        more: false,
        request: ui.request,
        parent: ui.parent(),
        offset: ui.offset,
        entries: Arc::new(vec![other, selected.clone()]),
        error: None,
    });
    ui.apply(&state);
    assert_eq!(ui.cursor, 1);
    assert_eq!(ui.page[ui.cursor].id, selected.id);
}

#[test]
fn failed_or_redundant_location_switches_leave_navigation_intact() {
    let (mut ui, state) = fixture();
    let (send, recv) = sync_channel(1);
    let cancel = AtomicBool::new(false);
    ui.key(key(KeyCode::Char('1')), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    send.send(Command::Refresh).unwrap();
    ui.key(key(KeyCode::Char('2')), &state, &send, &cancel);
    assert!(!ui.in_bin());
    assert!(ui.other_location.is_none());
    assert!(!ui.loading);
    assert!(ui.notice.as_ref().unwrap().0.contains("Please wait"));
}

#[test]
fn space_keeps_focus_and_enter_opens_group_actions() {
    let (mut ui, state) = fixture();
    let (send, recv) = sync_channel(16);
    let cancel = AtomicBool::new(false);
    ui.page = Arc::new(
        (0..2)
            .map(|i| Entry {
                id: 10 + i,
                name: format!("{i}.txt"),
                folder: false,
                size: 7,
                modified: 0,
                shared: false,
                exported: false,
                parent: 0,
                fingerprint: String::new(),
            })
            .collect(),
    );
    ui.key(key(KeyCode::Char(' ')), &state, &send, &cancel);
    assert_eq!(ui.cursor, 0);
    ui.key(key(KeyCode::Char(' ')), &state, &send, &cancel);
    assert!(ui.marked.is_empty());
    assert_eq!(ui.cursor, 0);
    ui.key(key(KeyCode::Char(' ')), &state, &send, &cancel);
    ui.key(key(KeyCode::Down), &state, &send, &cancel);
    ui.key(key(KeyCode::Char(' ')), &state, &send, &cancel);
    assert_eq!(ui.marked.len(), 2);
    assert!(draw(&ui, &state, 100, 28).contains("2 selected"));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(draw(&ui, &state, 100, 28).contains("Download"));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(matches!(&ui.modal,Some(Modal::Download{entries,..}) if entries.len()==2));
    ui.key(key(KeyCode::Tab), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(
        matches!(recv.try_recv().unwrap(),Command::DownloadMany{entries,..} if entries.len()==2)
    );
    assert!(ui.marked.is_empty());
    assert!(!ui.closed);
    assert!(!cancel.load(Ordering::Relaxed));
}

#[test]
fn preview_is_explicit_and_closing_it_cancels_only_the_preview() {
    let (mut ui, state) = fixture();
    let (send, recv) = sync_channel(16);
    let cancel = AtomicBool::new(false);
    ui.page = Arc::new(vec![Entry {
        id: 2,
        name: "notes.txt".into(),
        folder: false,
        size: 7,
        modified: 0,
        shared: false,
        exported: false,
        parent: 0,
        fingerprint: String::new(),
    }]);
    ui.key(key(KeyCode::Down), &state, &send, &cancel);
    ui.key(key(KeyCode::Char(' ')), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    ui.key(key(KeyCode::Char('p')), &state, &send, &cancel);
    let Command::Preview { request, .. } = recv.try_recv().unwrap() else {
        panic!()
    };
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    assert!(matches!(recv.try_recv().unwrap(),Command::CancelPreview(id) if id==request));
    assert!(!ui.closed);
    assert!(!cancel.load(Ordering::Relaxed));
    ui.page = Arc::new(vec![Entry {
        id: 3,
        name: "oversize.jpg".into(),
        folder: false,
        size: crate::preview::IMAGE_LIMIT + 1,
        modified: 0,
        shared: false,
        exported: false,
        parent: 0,
        fingerprint: String::new(),
    }]);
    ui.key(key(KeyCode::Char('p')), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(ui.modal.is_none());
    assert!(ui.notice.as_ref().unwrap().0.contains("100 MiB"));
    ui.page = fixture().0.page;
    ui.key(key(KeyCode::Char('p')), &state, &send, &cancel);
    assert!(
        ui.modal.is_none()
            && ui
                .notice
                .as_ref()
                .unwrap()
                .0
                .contains("Enter opens this folder")
    );
    assert!(recv.try_recv().is_err());
}

#[test]
fn transfer_menu_cancels_its_selected_job_without_exiting_or_cancelling_another() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    for id in 1..=2 {
        state.jobs.push_back(JobView {
            items: vec![
                TransferItem {
                    name: "first.wav".into(),
                    folder: false,
                    size: Some(7),
                },
                TransferItem {
                    name: "second.flac".into(),
                    folder: false,
                    size: Some(8192),
                },
            ]
            .into(),
            current_item: Some(0),
            id,
            direction: Direction::Download,
            name: "first.wav (+1)".into(),
            file: String::new(),
            destination: format!("/target-{id}"),
            state: JobState::Transferring,
            bytes: 1,
            total: 7,
            speed: 1.0,
            files: 0,
            folders: 0,
            error: None,
        });
    }
    ui.key(key(KeyCode::Char('t')), &state, &send, &cancel);
    let list = draw(&ui, &state, 80, 24);
    for text in [
        "first.wav",
        "second.flac",
        "8.0 KiB",
        "1 B of 7 B",
        "/target-1",
        "Cancel batch",
        "Cancel all",
        "Esc close",
    ] {
        assert!(list.contains(text), "{text}");
    }
    assert!(!list.contains("Enter details") && !list.contains("0 files complete"));
    ui.key(key(KeyCode::Down), &state, &send, &cancel);
    ui.key(key(KeyCode::Down), &state, &send, &cancel);
    assert!(draw(&ui, &state, 80, 24).contains("/target-2"));
    state.jobs.pop_front();
    ui.key(key(KeyCode::Char('x')), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    assert!(draw(&ui, &state, 80, 24).contains("Keep transferring"));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    // Cancel all still asks for confirmation when only one transfer remains.
    ui.key(key(KeyCode::Char('X')), &state, &send, &cancel);
    assert!(draw(&ui, &state, 80, 24).contains("Keep transferring"));
    ui.key(key(KeyCode::Right), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(matches!(recv.try_recv().unwrap(), Command::Cancel(ids) if ids == [2]));
    let mut next = state.jobs[0].clone();
    next.id = 3;
    state.jobs.push_back(next.clone());
    ui.key(key(KeyCode::Char('X')), &state, &send, &cancel);
    assert!(draw(&ui, &state, 80, 24).contains("Cancel all transfers?"));
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
    ui.key(key(KeyCode::Char('X')), &state, &send, &cancel);
    // A late arrival is outside the set the user reviewed.
    next.id = 4;
    state.jobs.push_back(next);
    ui.key(key(KeyCode::Right), &state, &send, &cancel);
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(matches!(recv.try_recv().unwrap(), Command::Cancel(ids) if ids == [2,3]));
    assert!(recv.try_recv().is_err());
    assert!(matches!(ui.modal, Some(Modal::Transfers { .. })));
    ui.key(key(KeyCode::Esc), &state, &send, &cancel);
    assert!(ui.modal.is_none());
    assert!(!ui.closed);
    assert!(!cancel.load(Ordering::Relaxed));
    ui.key(key(KeyCode::Char('t')), &state, &send, &cancel);
    state.jobs.clear();
    state.revision += 1;
    ui.apply(&state);
    let empty = draw(&ui, &state, 50, 18);
    assert!(empty.contains("No transfers"));
    assert!(!empty.contains("first.wav") && !empty.contains("Cancel batch"));
    ui.key(key(KeyCode::Char('x')), &state, &send, &cancel);
    assert!(recv.try_recv().is_err());
}

#[test]
fn help_is_grouped_and_colored_and_masked_identity_is_visible_in_both_layouts() {
    let (mut ui, mut state) = fixture();
    state.account = Some(crate::preview::redact_account("robin42@example.com"));
    for width in [70, 110] {
        assert!(draw(&ui, &state, width, 28).contains("ro…42@ex…om"));
    }
    ui.modal = Some(Modal::Help);
    let mut term = Terminal::new(TestBackend::new(110, 28)).unwrap();
    term.draw(|f| view::draw(f, &ui, &state)).unwrap();
    let buffer = term.backend().buffer();
    let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
    for label in [
        "Browse",
        "Select & act",
        "Transfers & navigation",
        "Space",
        "Preview focused file",
        "Choose download folder",
        "Choose files to upload",
        "Shift+X cancel all",
        "Item info and actions, including folders",
        "Switch file icons between Nerd Font and plain",
    ] {
        assert!(text.contains(label), "Missing {label}");
    }
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Indexed(81))
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Indexed(78))
    );
}

#[test]
#[ignore = "requires explicitly injected isolated live-account credentials"]
fn live_cloud_navigation() -> anyhow::Result<()> {
    use crate::{backend::Backend, state::State};
    use zeroize::Zeroizing;
    let user = Zeroizing::new(
        std::env::var("MEGAWREN_TEST_USER")
            .map_err(|_| anyhow::anyhow!("Live test user was not injected"))?,
    );
    let password = Zeroizing::new(
        std::env::var("MEGAWREN_TEST_PASSWORD")
            .map_err(|_| anyhow::anyhow!("Live test password was not injected"))?,
    );
    let scratch = tempfile::tempdir()?;
    let state = State::at(scratch.path().join("state"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let mut remote = Backend::new(&state, cancel.clone())?;
    let result = (|| -> anyhow::Result<()> {
        remote.login(&user, &password)?;
        remote.load_drive()?;
        let root = remote.api().root()?;
        let rubbish = remote.api().rubbish()?;
        anyhow::ensure!(root != rubbish, "Cloud and bin roots must differ");
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let mut engine = worker::Engine::new(&mut remote, shared.clone());
        let mut ui = Ui::new(root)?;
        let (send, recv) = sync_channel(8);
        for code in [
            KeyCode::Char('2'),
            KeyCode::Left,
            KeyCode::Char('2'),
            KeyCode::Esc,
            KeyCode::Char('2'),
            KeyCode::Char('1'),
        ] {
            let before = snapshot(&shared);
            ui.key(key(code), &before, &send, &cancel);
            while let Ok(command) = recv.try_recv() {
                engine.command(command);
            }
            engine.tick();
            let after = snapshot(&shared);
            ui.apply(&after);
            anyhow::ensure!(
                !ui.loading && after.page.as_ref().is_some_and(|p| p.error.is_none()),
                "Live location listing failed"
            );
            anyhow::ensure!(
                ui.parent()
                    == if code == KeyCode::Char('2') {
                        rubbish
                    } else {
                        root
                    },
                "Live navigation reached the wrong location"
            );
            let screen = draw(&ui, &after, 70, 24);
            anyhow::ensure!(
                screen.contains("1 Cloud drive") && screen.contains("2 Rubbish Bin"),
                "Location navigation is not visible"
            );
        }
        Ok(())
    })();
    let revoke = if remote.authenticated {
        remote.revoke_on_exit()
    } else {
        Ok(())
    };
    let finish = remote.finish();
    result?;
    revoke?;
    finish?;
    Ok(())
}

#[test]
fn long_removal_summaries_scroll_without_extra_confirmation_steps() {
    let (mut ui, mut state) = fixture();
    let (send, recv) = sync_channel(8);
    let cancel = AtomicBool::new(false);
    ui.review_trash(&send);
    let Command::InspectMany { request, .. } = recv.try_recv().unwrap() else {
        panic!()
    };
    let mut review = (*ready(&ui, request).details.unwrap()).clone();
    review.path = format!("/{}/folder", "very-deep/".repeat(200));
    state.details = Some(Inspection {
        request,
        details: Some(Arc::new(review)),
        error: None,
    });
    draw(&ui, &state, 100, 28);
    assert!(ui.detail_scroll_limit.get() > 0);
    ui.key(key(KeyCode::PageDown), &state, &send, &cancel);
    assert!(matches!(ui.modal,Some(Modal::Action(Dialog::TrashReview{scroll,..})) if scroll>0));
    ui.key(key(KeyCode::Enter), &state, &send, &cancel);
    assert!(matches!(
        recv.try_recv().unwrap(),
        Command::CancelInspect(_)
    ));
}

#[test]
fn release_folder_enter_navigates_and_listing_failure_is_distinct() {
    let (mut ui, mut state) = fixture();
    let (send, receive) = sync_channel(4);
    ui.key(key(KeyCode::Enter), &state, &send, &AtomicBool::new(false));
    let Command::List {
        parent, request, ..
    } = receive.try_recv().unwrap()
    else {
        panic!("Enter must navigate");
    };
    assert_eq!(parent, 3);
    state.revision += 1;
    state.page = Some(Page {
        more: false,
        request,
        parent,
        offset: 0,
        entries: Arc::default(),
        error: Some("Offline".into()),
    });
    ui.apply(&state);
    let screen = draw(&ui, &state, 80, 24);
    assert!(screen.contains("Offline"));
    assert!(!screen.contains("folder is empty"));
}
#[test]
fn release_failed_list_send_leaves_navigation_usable() {
    let (mut ui, _) = fixture();
    let (send, _receive) = sync_channel(0);
    ui.list(&send);
    assert!(!ui.loading);
}

#[test]
fn selection_sheet_distinguishes_folder_contents_from_known_file_sizes() {
    let cases: &[(&[Option<i64>], &str, &str)] = &[
        (
            &[None, None],
            "2 folders selected",
            "Folder sizes not calculated",
        ),
        (
            &[None, Some(12)],
            "1 file and 1 folder selected",
            "12 B excluding folder contents",
        ),
        (
            &[None, Some(0), Some(0)],
            "2 files and 1 folder selected",
            "0 B excluding folder contents",
        ),
        (&[Some(5), Some(7)], "2 files selected", "12 B total"),
        (&[Some(0), Some(0)], "2 files selected", "0 B total"),
        (
            &[Some(-1), Some(7)],
            "2 files selected",
            "File sizes unavailable",
        ),
        (
            &[Some(i64::MAX), Some(1)],
            "2 files selected",
            "File sizes unavailable",
        ),
    ];
    for &(sizes, title, summary) in cases {
        let (mut ui, state) = fixture();
        let template = ui.selection().unwrap();
        ui.marked = sizes
            .iter()
            .enumerate()
            .map(|(index, size)| {
                let mut item = template.clone();
                item.entry.id = index as u64 + 10;
                item.entry.folder = size.is_none();
                item.entry.size = size.unwrap_or(-1);
                item
            })
            .collect();
        let (send, recv) = sync_channel(4);
        ui.key(key(KeyCode::Enter), &state, &send, &AtomicBool::new(false));
        let screen = draw(&ui, &state, 80, 24);
        assert!(
            screen.contains(title) && screen.contains(summary),
            "{title}: {summary}"
        );
        assert!(
            recv.try_recv().is_err(),
            "Opening selection actions should use the listing"
        );
    }
}

#[test]
fn item_panel_uses_fresh_link_metadata_and_keeps_actions_visible() {
    let (mut ui, mut state) = fixture();
    let mut file = ui.page[0].clone();
    file.folder = false;
    file.name = "private.txt".into();
    file.exported = true; // Older directory response, before link removal.
    ui.page = Arc::new(vec![file]);
    let (send, recv) = sync_channel(4);
    ui.key(key(KeyCode::Enter), &state, &send, &AtomicBool::new(false));
    let Command::Inspect { request, .. } = recv.try_recv().unwrap() else {
        panic!("Expected fresh metadata")
    };
    for (status, label) in [
        (crate::bridge::ffi::LinkState::None, "No direct link"),
        (crate::bridge::ffi::LinkState::Expired, "Expired"),
        (crate::bridge::ffi::LinkState::TakenDown, "Disabled by MEGA"),
        (
            crate::bridge::ffi::LinkState::Active,
            "Anyone with the complete link",
        ),
    ] {
        let mut data = ready(&ui, request);
        Arc::make_mut(data.details.as_mut().unwrap()).direct_link = status;
        state.details = Some(data);
        let screen = draw(&ui, &state, 80, 24);
        assert!(screen.contains(label) && screen.contains("Owner") && screen.contains("Download"));
    }
}
