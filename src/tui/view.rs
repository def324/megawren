use super::{Modal, Ui, model::*, picker::Picker};
use ratatui::{
    Frame,
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, LineGauge, Padding, Paragraph, Row, Table,
        TableState, Wrap,
    },
};

// Extended palette avoids terminal themes remapping ANSI green to yellow.
const ACCENT: Color = Color::Indexed(81);
const SECONDARY: Color = Color::Reset;
const SUCCESS: Color = Color::Indexed(78);
const WARNING: Color = Color::Indexed(214);
const ERROR: Color = Color::Indexed(203);
const BORDER: Color = Color::Indexed(240);
fn secondary() -> Style {
    Style::default().fg(SECONDARY)
}
fn accent() -> Style {
    Style::default().fg(ACCENT)
}
fn selected() -> Style {
    Style::default()
        .bg(Color::Indexed(238))
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}
fn block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .title(
            title.into().style(
                Style::default()
                    .fg(Color::Reset)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
}
fn job_color(state: JobState) -> Color {
    match state {
        JobState::Complete => SUCCESS,
        JobState::Failed => ERROR,
        JobState::Cancelling | JobState::Waiting => WARNING,
        JobState::Transferring => ACCENT,
        _ => SECONDARY,
    }
}
fn job_progress(job: &JobView) -> String {
    if job.state == JobState::Complete {
        "Done".into()
    } else if job.total > 0 {
        format!("{}%", percent(job.bytes, job.total))
    } else {
        "—".into()
    }
}
fn transfer_rate(job: &JobView) -> String {
    let mut text = format!("{}/s", size(job.speed as i64));
    if job.files > 0 {
        text.push_str(&format!(
            " · {} {} saved",
            job.files,
            if job.files == 1 { "file" } else { "files" }
        ));
    }
    text
}
fn file_icon(name: &str, folder: bool, nerd: bool) -> Span<'static> {
    let (icon, color) = super::icons::lookup(name, folder, nerd);
    let padding = 3usize.saturating_sub(Span::raw(icon).width());
    Span::styled(
        format!("{icon}{}", " ".repeat(padding)),
        Style::default().fg(color),
    )
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
pub fn draw(frame: &mut Frame<'_>, ui: &Ui, state: &Snapshot) {
    let area = frame.area();
    ui.review_visible.set(area.width >= 72 && area.height >= 24);
    if area.width < 38 || area.height < 14 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal to browse files.\nCtrl-C exits.")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let vertical = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" megawren ", accent().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::raw(safe(&ui.path())),
        ])),
        vertical[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                " 1 Cloud drive ",
                if ui.in_bin() { secondary() } else { selected() },
            ),
            Span::raw("  "),
            Span::styled(
                " 2 Rubbish Bin ",
                if ui.in_bin() { selected() } else { secondary() },
            ),
        ])),
        Rect::new(vertical[0].x, vertical[0].y + 1, vertical[0].width, 1),
    );
    if area.width >= 88 {
        let columns = Layout::default()
            .direction(LayoutDirection::Horizontal)
            .constraints([
                Constraint::Min(42),
                Constraint::Length(1),
                Constraint::Length(32),
            ])
            .split(vertical[1]);
        browser(frame, ui, columns[0]);
        status(frame, state, columns[2]);
    } else {
        let rows = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(if state.jobs.iter().any(|j| !j.state.terminal()) {
                6
            } else {
                4
            }),
        ])
        .split(vertical[1]);
        browser(frame, ui, rows[0]);
        compact_status(frame, state, rows[1]);
    }
    let enter_hint = if ui.page.get(ui.cursor).is_some_and(|entry| entry.folder) {
        "Enter open folder"
    } else {
        "Enter actions"
    };
    let footer = if let Some((message, _)) = &ui.notice {
        vec![
            Line::styled(safe(message), accent()),
            Line::raw(if ui.marked.is_empty() {
                " ? help"
            } else {
                " Enter actions · Esc clear selection"
            }),
        ]
    } else if !ui.marked.is_empty() {
        vec![
            Line::styled(
                format!(" {} selected · Enter actions · Esc clear", ui.marked.len()),
                accent(),
            ),
            Line::raw(if ui.in_bin() {
                " d download · Enter for restore"
            } else {
                " d download · m move · c copy"
            }),
        ]
    } else if ui.in_bin() {
        vec![
            Line::styled(
                if ui.folders.len() == 1 {
                    " ← Cloud drive · Esc return"
                } else {
                    " ← parent · Esc Cloud drive"
                },
                accent(),
            ),
            Line::raw(safe(state.action.as_deref().unwrap_or(
                if ui.page.is_empty() {
                    " 1 Cloud drive · ? help"
                } else {
                    " i item info / actions · d download · ? help"
                },
            ))),
        ]
    } else if let Some(action) = &state.action {
        vec![
            Line::styled(action.clone(), accent()),
            Line::raw(" Browsing stays available · q quit"),
        ]
    } else if ui.page.is_empty() {
        vec![
            Line::raw(" u upload · n new folder"),
            Line::raw(" t transfers · ? help · q quit"),
        ]
    } else if area.width < 96 {
        vec![
            Line::raw(if area.width < 58 {
                " ↑↓ move  ← parent  → open  ? help"
            } else {
                " ↑↓ focus  ← parent  u upload  t transfers  ? help"
            }),
            Line::raw(format!(" {enter_hint} · Space select · q quit")),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled(" ↑↓", accent()),
                Span::raw(" focus  "),
                Span::styled("←", accent()),
                Span::raw(" parent  "),
                Span::styled("→", accent()),
                Span::raw(" open  "),
                Span::styled("d", accent()),
                Span::raw(" download  "),
                Span::styled("u", accent()),
                Span::raw(" upload  "),
                Span::styled("t", accent()),
                Span::raw(" transfers"),
            ]),
            Line::styled(
                format!(" {enter_hint} · Space select · i info · p preview · ? help · q quit"),
                secondary(),
            ),
        ]
    };
    frame.render_widget(Paragraph::new(footer), vertical[2]);
    if let Some(modal) = &ui.modal {
        frame
            .buffer_mut()
            .set_style(area, Style::default().fg(BORDER));
        modal_view(frame, modal, state, ui);
    }
}
fn browser(frame: &mut Frame<'_>, ui: &Ui, area: Rect) {
    let title = format!(
        " {} · {} items · page {} ",
        if ui.in_bin() {
            "Rubbish Bin"
        } else {
            "Cloud drive"
        },
        ui.page.len(),
        ui.offset / PAGE_SIZE + 1
    );
    if ui.loading {
        frame.render_widget(
            Paragraph::new("Loading files…")
                .style(secondary())
                .block(block(title)),
            area,
        );
        return;
    }
    if let Some(error) = &ui.list_error {
        frame.render_widget(
            Paragraph::new(format!("{}\nr retry · ← parent", safe(error)))
                .block(block(title))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if ui.page.is_empty() {
        frame.render_widget(
            Paragraph::new(if ui.in_bin() && ui.folders.len() == 1 {
                "Rubbish Bin is empty.\nPress ← or Esc to return to Cloud drive."
            } else if ui.in_bin() {
                "This folder is empty.\nPress ← for parent, or Esc for Cloud drive."
            } else {
                "This folder is empty. Press u to upload or n to create a folder."
            })
            .wrap(Wrap { trim: false })
            .style(secondary())
            .block(block(title)),
            area,
        );
        return;
    }
    let rows = ui.page.iter().map(|e| {
        let (_, kind) = kind(&e.name, e.folder);
        Row::new(vec![
            Cell::from(Line::from(vec![
                file_icon(&e.name, e.folder, ui.nerd_icons),
                Span::styled(
                    if ui.marked.iter().any(|s| s.entry.id == e.id) {
                        "● "
                    } else {
                        "  "
                    },
                    accent(),
                ),
                Span::raw(safe(&e.name)),
            ])),
            Cell::from(if e.folder { "—".into() } else { size(e.size) }),
            Cell::from(kind),
            Cell::from(modified(e.modified)),
        ])
    });
    let widths = if area.width >= 64 {
        vec![
            Constraint::Min(18),
            Constraint::Length(11),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Min(12),
            Constraint::Length(9),
            Constraint::Length(0),
            Constraint::Length(0),
        ]
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(["Name", "Size", "Type", "Modified"])
                .style(secondary())
                .bottom_margin(1),
        )
        .block(block(title))
        .row_highlight_style(selected())
        .highlight_symbol("› ")
        .column_spacing(1);
    let mut viewport = TableState::default()
        .with_offset(ui.viewport.get())
        .with_selected(Some(ui.cursor));
    frame.render_stateful_widget(table, area, &mut viewport);
    ui.viewport.set(viewport.offset());
}
fn connection(state: &Snapshot) -> (&'static str, Color) {
    match state.connection {
        Connection::Checking => ("Checking connection", SECONDARY),
        Connection::Ready => ("Connected", SUCCESS),
        Connection::Waiting => ("Waiting for MEGA", WARNING),
        Connection::Expired => ("Session expired", ERROR),
    }
}
fn status(frame: &mut Frame<'_>, state: &Snapshot, area: Rect) {
    let outer = block(" Status ").padding(Padding::uniform(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.height == 0 {
        return;
    }
    let (label, color) = connection(state);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(label, Style::default().fg(color)),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height >= 2
        && let Some(account) = &state.account
    {
        frame.render_widget(
            Paragraph::new(format!("  {}", safe(account))).style(secondary()),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    if inner.height >= 6 {
        frame.render_widget(
            Paragraph::new("Storage").style(Style::default().add_modifier(Modifier::BOLD)),
            Rect::new(inner.x, inner.y + 3, inner.width, 1),
        );
        if let Some(storage) = state.storage {
            let used = percent(storage.used, storage.total);
            let color = if used >= 95 { WARNING } else { ACCENT };
            frame.render_widget(
                LineGauge::default()
                    .filled_style(Style::default().fg(color))
                    .unfilled_style(Style::default().fg(BORDER))
                    .ratio(used as f64 / 100.0)
                    .label(format!("{used}% used")),
                Rect::new(inner.x, inner.y + 4, inner.width, 1),
            );
            frame.render_widget(
                Paragraph::new(format!("{} of {}", size(storage.used), size(storage.total)))
                    .style(secondary()),
                Rect::new(inner.x, inner.y + 5, inner.width, 1),
            );
            if inner.height >= 7 {
                frame.render_widget(
                    Paragraph::new(format!(
                        "{} free",
                        size((storage.total - storage.used).max(0))
                    ))
                    .style(secondary()),
                    Rect::new(inner.x, inner.y + 6, inner.width, 1),
                );
            }
        } else {
            frame.render_widget(
                Paragraph::new(if state.storage_pending {
                    "Loading storage…"
                } else {
                    "Storage unavailable"
                })
                .style(secondary()),
                Rect::new(inner.x, inner.y + 4, inner.width, 1),
            );
        }
    }
    let mut y = inner.y + 8;
    if y >= inner.bottom() {
        return;
    }
    frame.render_widget(
        Paragraph::new(if state.action.is_some() {
            "File action"
        } else {
            "Transfers"
        })
        .style(Style::default().add_modifier(Modifier::BOLD)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    if let Some(action) = &state.action {
        frame.render_widget(
            Paragraph::new(action.as_str())
                .style(accent())
                .wrap(Wrap { trim: true }),
            Rect::new(inner.x, y, inner.width, inner.bottom().saturating_sub(y)),
        );
        return;
    }
    if let Some(job) = state
        .jobs
        .iter()
        .find(|j| !j.state.terminal() && j.state != JobState::Queued)
    {
        let lines = vec![
            Line::styled(
                format!("{} {}", job.direction.icon(), job.direction.verb()),
                accent(),
            ),
            Line::raw(safe(if job.file.is_empty() {
                &job.name
            } else {
                &job.file
            })),
        ];
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(
                inner.x,
                y,
                inner.width,
                2.min(inner.bottom().saturating_sub(y)),
            ),
        );
        y += 2;
        if y < inner.bottom() {
            if job.state == JobState::Transferring && job.total > 0 {
                frame.render_widget(
                    Gauge::default()
                        .gauge_style(accent())
                        .percent(percent(job.bytes, job.total))
                        .label(format!("{}%", percent(job.bytes, job.total))),
                    Rect::new(inner.x, y, inner.width, 1),
                );
            } else {
                frame.render_widget(
                    Paragraph::new(job.state.label())
                        .style(Style::default().fg(job_color(job.state))),
                    Rect::new(inner.x, y, inner.width, 1),
                );
            }
            y += 1;
        }
        if y < inner.bottom() {
            frame.render_widget(
                Paragraph::new(transfer_rate(job)).style(secondary()),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 2;
        }
    } else if state.jobs.is_empty() {
        frame.render_widget(
            Paragraph::new("No active transfers"),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }
    let queued = state
        .jobs
        .iter()
        .filter(|j| j.state == JobState::Queued)
        .collect::<Vec<_>>();
    if !queued.is_empty() && y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(format!("{} queued", queued.len())).style(secondary()),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        for job in queued.iter().take(3) {
            if y >= inner.bottom() {
                break;
            }
            frame.render_widget(
                Paragraph::new(format!("{} {}", job.direction.icon(), safe(&job.name))),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
        y += 1;
    }
    for job in state
        .jobs
        .iter()
        .rev()
        .filter(|j| j.state.terminal())
        .take(HISTORY)
    {
        if y >= inner.bottom() {
            break;
        }
        let icon = match job.state {
            JobState::Complete => "✓",
            JobState::Failed => "!",
            _ => "–",
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{icon} "),
                    Style::default().fg(job_color(job.state)),
                ),
                Span::raw(safe(&job.name)),
            ])),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        if let Some(error) = &job.error
            && y < inner.bottom()
        {
            frame.render_widget(
                Paragraph::new(safe(error)).style(Style::default().fg(ERROR)),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    }
}
fn compact_status(frame: &mut Frame<'_>, state: &Snapshot, area: Rect) {
    let (connection, color) = connection(state);
    let storage = state
        .storage
        .map(|s| format!(" · {} / {}", size(s.used), size(s.total)))
        .unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("● {connection}"), Style::default().fg(color)),
        Span::raw(storage),
    ])];
    if let Some(account) = &state.account {
        lines.push(Line::styled(safe(account), secondary()));
    }
    if let Some(job) = state.jobs.iter().find(|j| !j.state.terminal()) {
        lines.push(Line::raw(format!(
            "{} {} · {}",
            job.direction.icon(),
            safe(&job.name),
            if job.total > 0 {
                format!("{}%", percent(job.bytes, job.total))
            } else {
                job.state.label().into()
            }
        )));
        lines.push(Line::styled(
            format!("{} · t transfers", job.state.label()),
            secondary(),
        ));
    } else {
        lines.push(Line::styled("No active transfers", secondary()));
    }
    frame.render_widget(Paragraph::new(lines).block(block(" Status ")), area);
}
fn modal_view(frame: &mut Frame<'_>, modal: &Modal, state: &Snapshot, ui: &Ui) {
    match modal {
        Modal::Preview {
            request,
            scroll,
            column,
            image,
        } => super::file_preview::draw(
            frame,
            &ui.preview_rows,
            state,
            *request,
            *scroll,
            *column,
            image,
        ),
        Modal::Action(dialog) => action_view(frame, dialog, state, ui),
        Modal::Upload {
            picker,
            destination,
            ..
        } => picker_view(
            frame,
            picker,
            " Choose files to upload ",
            &format!("To MEGA: {}", safe(destination)),
            None,
            false,
            ui.nerd_icons,
        ),
        Modal::Download {
            picker,
            entries,
            overwrite,
        } => picker_view(
            frame,
            picker,
            " Choose download folder ",
            &if entries.len() == 1 {
                format!("Download: {}", safe(&entries[0].name))
            } else {
                format!("Download {} selected items", entries.len())
            },
            Some(entries),
            *overwrite,
            ui.nerd_icons,
        ),
        Modal::Transfers {
            selected,
            cancellation,
        } => {
            if let Some(prompt) = cancellation {
                let area = centered(frame.area(), 64, 8);
                frame.render_widget(Clear, area);
                let buttons = if prompt.confirm {
                    "  Keep transferring    [ Cancel ]"
                } else {
                    "[ Keep transferring ]    Cancel"
                };
                let title = if prompt.ids.len() > 1 {
                    " Cancel all transfers? "
                } else {
                    " Cancel transfer? "
                };
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::styled(safe(&prompt.subject), accent()),
                        Line::raw("Completed files are kept."),
                        Line::raw(""),
                        Line::styled(
                            buttons,
                            if prompt.confirm {
                                Style::default().fg(WARNING)
                            } else {
                                accent()
                            },
                        ),
                        Line::styled("← → choose · Enter confirm · Esc back", secondary()),
                    ])
                    .block(block(title)),
                    area,
                );
            } else {
                transfers(frame, state, *selected);
            }
        }
        Modal::Help => help_view(frame),
        Modal::Quit { confirm } => {
            let area = centered(frame.area(), 58, 7);
            frame.render_widget(Clear, area);
            let button = if state.action.is_some() && *confirm {
                "  Continue    [ Quit ]"
            } else if state.action.is_some() {
                "[ Continue ]    Quit"
            } else if *confirm {
                "  Continue    [ Cancel transfers and quit ]"
            } else {
                "[ Continue ]    Cancel transfers and quit"
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(if state.action.is_some() {
                        "A file action is still running; it may finish after exit."
                    } else {
                        "Transfers are still running."
                    }),
                    Line::raw(""),
                    Line::styled(button, accent()),
                    Line::styled("← → choose · Enter confirm · Esc continue", secondary()),
                ])
                .block(block(" Quit? ")),
                area,
            );
        }
        Modal::Error(message) => {
            let area = centered(frame.area(), 72, 9);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(format!("{}\n\nEnter or Esc closes", safe(message)))
                    .style(Style::default().fg(ERROR))
                    .block(block(" Cannot complete action "))
                    .wrap(Wrap { trim: true }),
                area,
            );
        }
    }
}
fn transfers(frame: &mut Frame<'_>, state: &Snapshot, focus: Option<TransferFocus>) {
    let count: usize = state.jobs.iter().map(|j| j.items.len()).sum();
    let area = centered(frame.area(), 88, (count as u16 + 10).clamp(6, 28));
    frame.render_widget(Clear, area);
    let outer = block(" Transfers ").padding(Padding::horizontal(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if count == 0 {
        frame.render_widget(Paragraph::new("No transfers"), inner);
        frame.render_widget(
            Paragraph::new("Esc close").style(secondary()),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
        return;
    }
    let show_size = inner.width >= 52;
    let mut info = Vec::new();
    let mut keys = Vec::new();
    if count > 1 {
        keys.push("↑↓ select");
    }
    let selected_row = state
        .transfer_rows()
        .position(|(row, _, _)| Some(row) == focus);
    if let Some((row, job, item)) = selected_row.and_then(|i| state.transfer_rows().nth(i)) {
        let direction = match job.direction {
            Direction::Upload => "Upload",
            Direction::Download => "Download",
        };
        let batch = if job.items.len() > 1 {
            format!(" · item {} of {}", row.item + 1, job.items.len())
        } else {
            String::new()
        };
        info.push(Line::from(vec![
            Span::styled(format!("{direction}{batch}"), accent()),
            Span::raw(format!(" → {}", safe(&job.destination))),
        ]));
        let selected_name = safe(&item.name);
        let name_width = inner.width.saturating_sub(if show_size { 32 } else { 21 });
        if Span::raw(&selected_name).width() > name_width as usize {
            info.push(Line::raw(selected_name));
        }
        if job.current_item == Some(row.item) {
            if item.folder && !job.file.is_empty() && job.file != item.name {
                info.push(Line::raw(safe(&job.file)));
            }
            if job.total > 0 && job.state != JobState::Complete {
                let mut detail = format!("{} of {}", size(job.bytes), size(job.total));
                if job.speed > 0.0 && !job.state.terminal() {
                    detail.push_str(&format!(" · {}/s", size(job.speed as i64)));
                }
                info.push(Line::styled(detail, accent()));
            }
        }
        if job.files > 0 || job.folders > 0 {
            info.push(Line::raw(job.completed()));
        }
        if let Some(error) = &job.error {
            info.push(Line::styled(safe(error), Style::default().fg(ERROR)));
        }
        if job.state.cancellable() {
            keys.push(if job.items.len() > 1 {
                "x Cancel batch"
            } else {
                "x Cancel transfer"
            });
        }
    }
    if state
        .jobs
        .iter()
        .filter(|job| job.state.cancellable())
        .count()
        > 1
    {
        keys.push("Shift+X Cancel all");
    }
    keys.push("Esc close");
    let hints = Paragraph::new(keys.join(" · "))
        .style(secondary())
        .wrap(Wrap { trim: true });
    let info = Paragraph::new(info).wrap(Wrap { trim: false });
    let footer_height = hints.line_count(inner.width) as u16;
    let summary_height =
        (info.line_count(inner.width) as u16).min(inner.height.saturating_sub(footer_height + 4));
    let [table, _, summary, footer] = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(summary_height),
        Constraint::Length(footer_height),
    ])
    .areas(inner);
    let capacity = table.height.saturating_sub(1) as usize;
    let start = selected_row
        .unwrap_or(0)
        .saturating_sub(capacity / 2)
        .min(count.saturating_sub(capacity));
    let rows = state
        .transfer_rows()
        .skip(start)
        .take(capacity)
        .map(|(row, job, item)| {
            let active = job.current_item == Some(row.item);
            let (label, color) = if job.state == JobState::Complete
                || job.current_item.is_some_and(|i| row.item < i)
            {
                ("Complete".into(), SUCCESS)
            } else if job.state == JobState::Failed && !active && job.current_item.is_some() {
                ("Skipped".into(), SECONDARY)
            } else if active || job.state.terminal() || job.state == JobState::Cancelling {
                let text = if job.state == JobState::Transferring && job.total > 0 {
                    job_progress(job)
                } else {
                    job.state.label().into()
                };
                (text, job_color(job.state))
            } else {
                ("Queued".into(), SECONDARY)
            };
            let mut cells = vec![Cell::from(format!(
                "{} {}{}",
                job.direction.icon(),
                safe(&item.name),
                if item.folder { "/" } else { "" }
            ))];
            if show_size {
                cells.push(Cell::from(item.size.map(size).unwrap_or_default()));
            }
            cells.push(Cell::from(label).style(Style::default().fg(color)));
            Row::new(cells)
        });
    let mut columns = vec![Constraint::Fill(1)];
    let mut headers = vec!["Name"];
    if show_size {
        columns.push(Constraint::Length(9));
        headers.push("Size");
    }
    columns.push(Constraint::Length(16));
    headers.push("Status");
    frame.render_stateful_widget(
        Table::new(rows, columns)
            .header(Row::new(headers).style(secondary()))
            .row_highlight_style(selected())
            .highlight_symbol("› "),
        table,
        &mut TableState::default().with_selected(selected_row.map(|i| i - start)),
    );
    frame.render_widget(info, summary);
    frame.render_widget(hints, footer);
}

fn help_view(frame: &mut Frame<'_>) {
    let area = centered(frame.area(), 76, 24);
    frame.render_widget(Clear, area);
    let outer = block(" Browser shortcuts ")
        .title_bottom(Line::styled(" ? / Esc close ", secondary()))
        .padding(Padding::horizontal(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let heading = |text: &'static str| Line::styled(text, accent().add_modifier(Modifier::BOLD));
    let binding = |keys: &'static str, description: &'static str| {
        Line::from(vec![
            Span::styled(format!(" {keys:<15}"), Style::default().fg(SUCCESS)),
            Span::raw(description),
        ])
    };
    if inner.height < 20 || inner.width < 58 {
        frame.render_widget(
            Paragraph::new(vec![
                heading("Keys"),
                Line::raw("↑↓ focus · → open · ← parent"),
                Line::raw("Enter open folder / file actions"),
                Line::raw("Space mark · i info · p preview"),
                Line::raw("d download · u upload"),
                Line::raw("1 Drive · 2 Rubbish Bin"),
                Line::raw("t transfers · q quit"),
                Line::raw("F3 icon style in file lists"),
            ]),
            inner,
        );
        return;
    }
    let lines = vec![
        heading("Browse"),
        binding("↑↓  → ←", "Focus · open folder · parent"),
        binding("1 / 2", "Cloud drive / Rubbish Bin"),
        Line::raw(""),
        heading("Select & act"),
        binding("Space", "Toggle selection"),
        binding("Enter", "Open folder / actions for files or marked items"),
        binding("i", "Item info and actions, including folders"),
        binding("p", "Preview focused file · on request only"),
        binding("d", "Choose download folder"),
        binding("u", "Choose files to upload"),
        binding("m / c / F2", "Move / copy / rename"),
        binding("Delete / n", "Move to Rubbish Bin / new folder"),
        binding("Esc", "Close dialog / clear selection / leave bin"),
        Line::raw(""),
        heading("Transfers & navigation"),
        binding("t", "Transfers · x cancel · Shift+X cancel all"),
        binding("PgUp / PgDn", "File pages · preview scroll"),
        binding("r", "Refresh folder"),
        binding("F3", "Switch file icons between Nerd Font and plain"),
        binding("q / Ctrl-C", "Quit / interrupt"),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}
fn selection_summary(targets: &[Selection]) -> (String, String) {
    let folders = targets.iter().filter(|s| s.entry.folder).count();
    let files = targets.len() - folders;
    if files == 0 {
        return (
            format!("{folders} folders selected"),
            "Folder sizes not calculated".into(),
        );
    }
    let file_word = if files == 1 { "file" } else { "files" };
    let title = if folders == 0 {
        format!("{files} files selected")
    } else {
        let folder_word = if folders == 1 { "folder" } else { "folders" };
        format!("{files} {file_word} and {folders} {folder_word} selected")
    };
    let bytes = targets
        .iter()
        .filter(|s| !s.entry.folder)
        .try_fold(0i64, |sum, s| {
            let bytes = s.entry.size;
            if bytes < 0 {
                None
            } else {
                sum.checked_add(bytes)
            }
        });
    let summary = match bytes {
        Some(n) if folders == 0 => format!("{} total", size(n)),
        Some(n) => format!("{} excluding folder contents", size(n)),
        None => "File sizes unavailable".into(),
    };
    (title, summary)
}

fn action_view(frame: &mut Frame<'_>, dialog: &super::actions::Dialog, state: &Snapshot, ui: &Ui) {
    use super::actions::{Dialog, NameKind};
    let area = centered(frame.area(), 90, 27);
    match dialog {
        Dialog::Menu {
            cursor,
            request,
            targets,
        } => {
            let choices = super::actions::item_actions(targets, ui.in_bin());
            let mut info = Vec::new();
            let title = match targets.as_slice() {
                [] => "Folder actions".into(),
                [item] => safe(&item.entry.name),
                many => {
                    let (title, summary) = selection_summary(many);
                    info.push(Line::styled(summary, secondary()));
                    title
                }
            };
            if let [item] = targets.as_slice() {
                let inspection = state
                    .details
                    .as_ref()
                    .filter(|i| Some(i.request) == *request);
                let details = inspection
                    .and_then(|i| i.details.as_ref())
                    .filter(|d| d.selection.entry.id == item.entry.id);
                info.push(Line::styled(
                    if item.entry.folder {
                        "Folder".into()
                    } else {
                        format!(
                            "{} · {}",
                            kind(&item.entry.name, false).1,
                            size(item.entry.size)
                        )
                    },
                    accent(),
                ));
                if let Some(d) = details {
                    info.push(Line::raw(safe(&d.path)));
                    if let Some(s) = &d.stats {
                        info.push(Line::raw(format!(
                            "{} files · {} folders · {}",
                            s.files,
                            s.folders,
                            size(s.bytes)
                        )));
                        if s.versions > 0 {
                            info.push(Line::raw(format!(
                                "Previous versions  {} · {}",
                                s.versions,
                                size(s.version_bytes)
                            )));
                        }
                    }
                    info.push(Line::raw(format!(
                        "Created  {}",
                        crate::bridge::ffi::utc_date(d.created)
                    )));
                    if !item.entry.folder {
                        info.push(Line::raw(format!(
                            "Modified  {}",
                            crate::bridge::ffi::utc_date(item.entry.modified)
                        )));
                    }
                    info.push(Line::raw(format!(
                        "Your access  {}",
                        access_label(d.access)
                    )));
                    info.push(Line::raw(format!(
                        "Share link  {}",
                        link_label(d.direct_link)
                    )));
                    if d.in_rubbish && d.restore.is_none() {
                        info.push(Line::raw("Original folder unavailable"));
                    } else if d.restricted && !d.in_rubbish {
                        info.push(Line::raw("Cloud changes require a private item you own"));
                    }
                    if item.entry.shared {
                        info.push(Line::raw("Shared with people"));
                    }
                    if d.favourite {
                        info.push(Line::raw("Favourite"));
                    }
                } else if let Some(error) = inspection.and_then(|i| i.error.as_ref()) {
                    info.push(Line::styled(safe(error), secondary()));
                } else if request.is_some() {
                    info.push(Line::styled("Reading details…", secondary()));
                }
            }
            let height = (info.len() + choices.len() + 5) as u16;
            let area = centered(frame.area(), 68, height);
            frame.render_widget(Clear, area);
            let border = block(format!(" {title} ")).title_bottom(Line::styled(
                " ↑↓ choose · Enter select · Esc close ",
                secondary(),
            ));
            let inner = border.inner(area);
            frame.render_widget(border, area);
            // Preserve action rows on short terminals.
            let rows =
                (info.len() as u16).min(inner.height.saturating_sub(choices.len() as u16 + 1));
            let parts = Layout::vertical([
                Constraint::Length(rows),
                Constraint::Length(u16::from(rows > 0)),
                Constraint::Min(1),
            ])
            .split(inner);
            frame.render_widget(Paragraph::new(info), parts[0]);
            let items = choices.into_iter().map(|action| {
                let (label, key) = action.binding();
                Row::new([
                    Cell::from(label).style(if action == super::actions::ItemAction::Trash {
                        Style::default().fg(WARNING)
                    } else {
                        Style::default()
                    }),
                    Cell::from(key.map(|code| code.to_string()).unwrap_or_default())
                        .style(accent()),
                ])
            });
            frame.render_stateful_widget(
                Table::new(items, [Constraint::Min(16), Constraint::Length(6)])
                    .row_highlight_style(selected())
                    .highlight_symbol("› "),
                parts[2],
                &mut TableState::default().with_selected(Some(*cursor)),
            );
        }
        Dialog::Name {
            kind,
            selection,
            value,
            error,
            location,
            ..
        } => {
            let area = centered(frame.area(), 80, 10);
            frame.render_widget(Clear, area);
            let title = match kind {
                NameKind::Rename => " Rename ",
                NameKind::NewFolder => " New folder ",
                NameKind::Copy => " Copy as ",
            };
            let mut lines = vec![];
            lines.push(Line::raw(format!("In: {}", safe(location))));
            if let Some(s) = selection {
                lines.push(Line::raw(format!("Item: {}", safe(&s.entry.name))));
            }
            lines.extend([
                Line::raw(""),
                Line::styled(format!("Name: {}▏", safe(value)), accent()),
                Line::raw(""),
                Line::raw(
                    error
                        .as_deref()
                        .unwrap_or("Enter confirm · Ctrl+U clear · Esc cancel"),
                ),
            ]);
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block(title))
                    .wrap(Wrap { trim: false }),
                area,
            );
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
            frame.render_widget(Clear, area);
            let title = if *copy {
                " Choose copy destination "
            } else {
                " Choose move destination "
            };
            let outer = block(title);
            let inner = outer.inner(area);
            frame.render_widget(outer, area);
            let parts = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(2),
                Constraint::Length(3),
            ])
            .split(inner);
            let path = folders
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join("/");
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(format!(
                        "Item: {}",
                        if selections.len() == 1 {
                            safe(&selections[0].entry.name)
                        } else {
                            format!("{} selected items", selections.len())
                        }
                    )),
                    Line::styled(
                        format!("To: /{}", safe(path.trim_start_matches('/'))),
                        accent(),
                    ),
                ]),
                parts[0],
            );
            let page = state.picker_page.as_ref().filter(|p| {
                p.request == *request
                    && p.parent == folders.last().unwrap().0
                    && p.offset == *offset
            });
            if let Some(page) = page {
                if let Some(error) = &page.error {
                    frame.render_widget(
                        Paragraph::new(safe(error)).wrap(Wrap { trim: true }),
                        parts[1],
                    );
                } else {
                    let rows = std::iter::once(Row::new([if folders.len() > 1 {
                        "↑  Parent folder"
                    } else {
                        "Cloud drive"
                    }]))
                    .chain(
                        page.entries
                            .iter()
                            .map(|e| Row::new([format!("▸  {}", safe(&e.name))])),
                    );
                    frame.render_stateful_widget(
                        Table::new(rows, [Constraint::Min(1)])
                            .row_highlight_style(selected())
                            .highlight_symbol("› "),
                        parts[1],
                        &mut TableState::default().with_selected((!*button).then_some(*cursor)),
                    );
                }
            } else {
                frame.render_widget(Paragraph::new("Loading folders…"), parts[1]);
            }
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        if *copy {
                            "[ c  Copy here ]"
                        } else {
                            "[ m  Move here ]"
                        },
                        if *button { selected() } else { accent() },
                    ),
                    Line::raw(if *button && *copy {
                        "Enter copy here · Esc cancel"
                    } else if *button {
                        "Enter move here · Esc cancel"
                    } else {
                        "Enter open folder · ← parent · Esc cancel"
                    }),
                ]),
                parts[2],
            );
        }
        Dialog::TrashReview {
            request,
            confirm,
            scroll,
            targets,
        } => trash_review(frame, ui, state, *request, *confirm, *scroll, targets),
    }
}
fn access_label(access: i32) -> &'static str {
    match access {
        0 => "Read only",
        1 => "Read and write",
        2 => "Full access",
        3 => "Owner",
        _ => "Unknown",
    }
}
fn link_label(link: crate::bridge::ffi::LinkState) -> &'static str {
    use crate::bridge::ffi::LinkState;
    match link {
        LinkState::Active => "Anyone with the complete link",
        LinkState::Expired => "Expired",
        LinkState::TakenDown => "Disabled by MEGA",
        LinkState::None => "No direct link",
        _ => "Unknown",
    }
}
fn trash_review(
    frame: &mut Frame<'_>,
    ui: &Ui,
    state: &Snapshot,
    request: u64,
    confirm: bool,
    scroll: u16,
    targets: &[Selection],
) {
    let area = centered(frame.area(), 90, 24);
    let outer = block(" Move to Rubbish Bin? ");
    let inner = outer.inner(area);
    if frame.area().width < 72 || frame.area().height < 24 {
        frame.render_widget(Clear, area);
        frame.render_widget(outer, area);
        frame.render_widget(
            Paragraph::new("Enlarge the terminal to review this selection.\nEsc cancel")
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let inspection = state.details.as_ref().filter(|d| d.request == request);
    let details = inspection.and_then(|d| d.details.as_ref());
    let mut lines = vec![];
    lines.push(Line::styled(
        format!(
            "{} {} selected",
            targets.len(),
            if targets.len() == 1 { "item" } else { "items" }
        ),
        accent().add_modifier(Modifier::BOLD),
    ));
    if let Some(d) = details {
        if d.review_ready {
            let members: Vec<_> = if d.members.is_empty() {
                vec![d.as_ref()]
            } else {
                d.members.iter().map(AsRef::as_ref).collect()
            };
            let (mut files, mut folders, mut bytes) = (0i64, 0i64, 0i64);
            for m in &members {
                if let Some(stats) = &m.stats {
                    files = files.saturating_add(stats.files);
                    folders = folders.saturating_add(stats.folders + 1);
                    bytes = bytes.saturating_add(stats.bytes);
                } else {
                    files += 1;
                    bytes = bytes.saturating_add(m.selection.entry.size.max(0));
                }
            }
            lines.push(Line::raw(format!(
                "{files} files · {folders} folders · {}",
                size(bytes)
            )));
            lines.push(Line::raw(""));
            for m in members {
                lines.push(Line::raw(safe(&m.path)));
            }
        } else {
            lines.push(Line::raw(format!(
                "Checking contents… {} / {} items · {} entries checked",
                d.members.len(),
                targets.len(),
                d.checked + d.members.iter().map(|item| item.checked).sum::<usize>()
            )));
            for target in targets {
                lines.push(Line::raw(safe(&target.entry.name)));
            }
        }
    }
    if let Some(error) = inspection.and_then(|d| d.error.as_deref()) {
        lines.push(Line::styled(safe(error), Style::default().fg(ERROR)));
    } else if details.is_none() {
        lines.push(Line::raw("Reading details…"));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = (paragraph.line_count(inner.width) + 7).clamp(11, 24) as u16;
    let area = centered(frame.area(), 90, height);
    let inner = outer.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(outer, area);
    let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(5)]).split(inner);
    let max_scroll = paragraph
        .line_count(parts[0].width)
        .saturating_sub(parts[0].height as usize)
        .min(u16::MAX as usize) as u16;
    ui.detail_scroll_limit.set(max_scroll);
    frame.render_widget(paragraph.scroll((scroll.min(max_scroll), 0)), parts[0]);
    let footer = if details.is_some_and(|d| d.review_ready) {
        vec![
            Line::raw("Items can be restored while they remain in the bin."),
            Line::raw("MEGA may automatically clear old bin items."),
            Line::styled(
                if confirm {
                    "   Cancel       [ Move to Rubbish Bin ]"
                } else {
                    "[ Cancel ]        Move to Rubbish Bin"
                },
                Style::default().fg(WARNING),
            ),
            Line::styled(
                "← → / Tab choose · Enter confirm · ↑↓ scroll · Esc cancel",
                secondary(),
            ),
        ]
    } else {
        vec![
            Line::raw("Removal stays blocked until review completes."),
            Line::styled("↑↓ scroll · Esc cancel", secondary()),
        ]
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), parts[1]);
}

fn picker_view(
    frame: &mut Frame<'_>,
    picker: &Picker,
    title: &'static str,
    summary: &str,
    download: Option<&[Entry]>,
    overwrite: bool,
    nerd_icons: bool,
) {
    let area = centered(frame.area(), 96, 26);
    frame.render_widget(Clear, area);
    let outer = block(title).padding(Padding::uniform(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(summary.to_owned()).style(Style::default().add_modifier(Modifier::BOLD)),
        parts[0],
    );
    let editing = picker.location_input.is_some();
    let location = picker
        .location_input
        .clone()
        .unwrap_or_else(|| picker.directory.display().to_string());
    let location_line = Line::from(vec![
        Span::styled(
            if download.is_some() {
                "Save in: "
            } else {
                "Browse: "
            },
            accent(),
        ),
        Span::raw(safe(&location)),
        Span::styled(if editing { "▏" } else { "" }, accent()),
    ]);
    let navigation = if editing {
        "Enter go · Ctrl+U clear · Esc keep current folder"
    } else {
        "← Parent folder · → Open folder · Ctrl+L Location · F3 Icons"
    };
    frame.render_widget(
        Paragraph::new(vec![location_line, Line::styled(navigation, secondary())])
            .wrap(Wrap { trim: false }),
        parts[1],
    );
    let rows = picker.entries.iter().map(|e| {
        let mark = if picker.directory_only || e.parent {
            ""
        } else if picker.selected.contains(&e.path) {
            "✓ "
        } else {
            "□ "
        };
        let (_, kind) = kind(&e.name, e.folder);
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled(mark, Style::default().fg(SUCCESS)),
                if e.parent {
                    Span::styled("↑  ", accent())
                } else {
                    file_icon(&e.name, e.folder, nerd_icons)
                },
                Span::raw(if e.parent {
                    "..  Parent folder".into()
                } else {
                    safe(&e.name)
                }),
            ])),
            Cell::from(if e.folder { "—".into() } else { size(e.size) }),
            Cell::from(if e.parent { "" } else { kind }),
        ])
    });
    let mut viewport = TableState::default()
        .with_offset(picker.viewport.get())
        .with_selected(Some(picker.cursor));
    frame.render_stateful_widget(
        Table::new(
            rows,
            if picker.directory_only {
                vec![
                    Constraint::Min(16),
                    Constraint::Length(0),
                    Constraint::Length(0),
                ]
            } else {
                vec![
                    Constraint::Min(16),
                    Constraint::Length(12),
                    Constraint::Length(10),
                ]
            },
        )
        .header(
            Row::new(if picker.directory_only {
                ["Folders on this computer", "", ""]
            } else {
                ["Name", "Size", "Type"]
            })
            .style(secondary()),
        )
        .row_highlight_style(if picker.button_focus {
            secondary()
        } else {
            selected()
        })
        .highlight_symbol("› "),
        parts[2],
        &mut viewport,
    );
    picker.viewport.set(viewport.offset());
    if let Some(entries) = download {
        let entry = &entries[0];
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(format!(
                    "Output: {}",
                    if entries.len() == 1 {
                        safe(&picker.directory.join(&entry.name).display().to_string())
                    } else {
                        format!(
                            "{} items in {}",
                            entries.len(),
                            safe(&picker.directory.display().to_string())
                        )
                    }
                )),
                Line::styled(
                    if !picker.conflict {
                        String::new()
                    } else if overwrite {
                        "☑ Replace matching files in selection · o to undo".into()
                    } else {
                        "Name already exists · o to allow replacement".into()
                    },
                    if overwrite {
                        Style::default().fg(WARNING)
                    } else {
                        secondary()
                    },
                ),
            ])
            .wrap(Wrap { trim: false }),
            parts[3],
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!(
                "{} selected · Space toggles selection",
                picker.selected.len()
            ))
            .style(secondary()),
            parts[3],
        );
    }
    let action = if download.is_some() {
        "Download to this folder".into()
    } else {
        format!("Upload {} selected", picker.selected.len())
    };
    let mut text = vec![Line::styled(
        format!(
            "[ {}  {action} ]",
            if download.is_some() { "d" } else { "u" }
        ),
        if picker.button_focus {
            selected()
        } else {
            accent()
        },
    )];
    if let Some(error) = &picker.error {
        text.push(Line::styled(safe(error), Style::default().fg(ERROR)));
    }
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), parts[4]);
    frame.render_widget(
        Paragraph::new(if editing {
            ""
        } else if picker.button_focus && download.is_some() {
            "Enter download here · Esc cancel"
        } else if picker.button_focus {
            "Enter upload selected · Esc cancel"
        } else if !picker.directory_only
            && picker.entries.get(picker.cursor).is_some_and(|e| !e.folder)
        {
            "Space select file · u upload selected · Esc cancel"
        } else if picker.more || picker.offset > 0 {
            "Enter open folder · PgUp/PgDn pages · Esc cancel"
        } else {
            "Enter open folder · Esc cancel"
        })
        .style(secondary()),
        parts[5],
    );
}

pub(super) fn draw_lifecycle(
    frame: &mut Frame<'_>,
    app: &super::lifecycle::AppState,
    prompt: Option<&super::lifecycle::Prompt>,
    elapsed: std::time::Duration,
) {
    use super::lifecycle::{InputMode, Phase};
    let prompt = if app.phase == Phase::Input {
        prompt
    } else {
        None
    };
    let area = frame.area();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(" megawren").style(accent().add_modifier(Modifier::BOLD)),
        rows[0],
    );
    if area.width < 38 || area.height < 14 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal. Ctrl-C exits.").wrap(Wrap { trim: true }),
            rows[1],
        );
        return;
    }
    let title = if let Some(p) = prompt {
        if p.mode() == InputMode::Error {
            "Could not continue"
        } else if p.mode() == InputMode::Choice {
            if p.label().starts_with("Remember") {
                "Remember login"
            } else {
                "Sign in again"
            }
        } else if p.label().starts_with("Unlock") {
            "Unlock saved login"
        } else if p.label().starts_with("Authenticator") {
            "Two-factor authentication"
        } else if p.label().contains("unlock passphrase") {
            "Remember login"
        } else {
            "Sign in to MEGA"
        }
    } else if app.closing {
        "Closing megawren"
    } else {
        "Opening cloud drive"
    };
    let card = centered(rows[1], 78, 16);
    let outer = block(format!(" {title} ")).padding(Padding::uniform(1));
    let inner = outer.inner(card);
    frame.render_widget(outer, card);
    if inner.height < 5 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal. Ctrl-C exits.").wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let hint;
    if let Some(p) = prompt {
        match p.mode() {
            InputMode::Error => {
                frame.render_widget(
                    Paragraph::new(p.label()).wrap(Wrap { trim: false }),
                    Rect::new(
                        inner.x,
                        inner.y,
                        inner.width,
                        inner.height.saturating_sub(2),
                    ),
                );
                hint = "Enter or Esc close";
            }
            InputMode::Choice => {
                let remember = p.label().starts_with("Remember");
                frame.render_widget(
                    Paragraph::new(p.label()).wrap(Wrap { trim: true }),
                    Rect::new(inner.x, inner.y, inner.width, 2),
                );
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            if remember {
                                "  No, this time only  "
                            } else {
                                "  No  "
                            },
                            if p.choice { accent() } else { selected() },
                        ),
                        Span::raw("   "),
                        Span::styled(
                            if remember {
                                "  Yes, save login  "
                            } else {
                                "  Yes, sign in again  "
                            },
                            if p.choice { selected() } else { accent() },
                        ),
                    ])),
                    Rect::new(inner.x, inner.y + 3, inner.width, 1),
                );
                hint = "← → choose · Enter confirm · Esc quit";
            }
            InputMode::Text => {
                let explanation = if p.label().starts_with("Unlock") {
                    "Enter the local passphrase for this device."
                } else if p.label().starts_with("New local") {
                    "Choose a local unlock passphrase of at least 12 characters."
                } else if p.label().starts_with("Confirm") {
                    "Enter the same local passphrase again."
                } else if p.label().starts_with("Authenticator") {
                    "Enter the code from your authenticator."
                } else {
                    "Your MEGA password and authenticator code are never saved."
                };
                frame.render_widget(
                    Paragraph::new(explanation).wrap(Wrap { trim: true }),
                    Rect::new(inner.x, inner.y, inner.width, 2),
                );
                frame.render_widget(
                    Paragraph::new(p.label().trim_end_matches([':', ' '])),
                    Rect::new(inner.x, inner.y + 2, inner.width, 1),
                );
                let text = p.display_value();
                let width = Span::raw(&text).width() as u16;
                let field = Rect::new(inner.x, inner.y + 3, inner.width, 2);
                frame.render_widget(
                    Paragraph::new(text)
                        .scroll((0, width.saturating_sub(inner.width.saturating_sub(1))))
                        .block(
                            Block::default()
                                .borders(Borders::BOTTOM)
                                .border_style(accent()),
                        ),
                    field,
                );
                frame.set_cursor_position((
                    inner.x + width.min(inner.width.saturating_sub(1)),
                    inner.y + 3,
                ));
                if let Some(error) = p.error {
                    frame.render_widget(
                        Paragraph::new(error)
                            .style(Style::default().fg(WARNING))
                            .wrap(Wrap { trim: true }),
                        Rect::new(
                            inner.x,
                            inner.y + 6,
                            inner.width,
                            2.min(inner.height.saturating_sub(6)),
                        ),
                    );
                }
                hint = if p.label().starts_with("Unlock") {
                    "Enter unlock · F2 sign in again · Esc quit"
                } else {
                    "Enter continue · Ctrl+U clear · Esc quit"
                };
            }
        }
    } else {
        let (message, progress) = match &app.phase {
            Phase::Busy { message, percent } => (message.as_str(), *percent),
            _ => ("Preparing…", None),
        };
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame_index = (elapsed.as_millis() / 100 % frames.len() as u128) as usize;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}  ", frames[frame_index]), accent()),
                Span::raw(safe(message)),
            ]))
            .wrap(Wrap { trim: true }),
            Rect::new(inner.x, inner.y + 1, inner.width, 2),
        );
        if let Some(progress) = progress {
            frame.render_widget(
                LineGauge::default()
                    .ratio(progress as f64 / 100.0)
                    .label("")
                    .filled_style(accent())
                    .unfilled_style(Style::default().fg(BORDER)),
                Rect::new(inner.x, inner.y + 4, inner.width, 1),
            );
        }
        hint = if app.closing {
            "Ctrl-C force exit"
        } else {
            "Esc or Ctrl-C quit"
        };
    }
    frame.render_widget(
        Paragraph::new(hint).style(accent()),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
    let location = if matches!(&app.phase, Phase::Busy {message, ..} if message.starts_with("Saving local index"))
    {
        format!(
            " Encrypted index: {}",
            safe(
                &std::path::Path::new(&app.state_path)
                    .join("cache.enc")
                    .display()
                    .to_string()
            )
        )
    } else {
        format!(" Local state: {}", safe(&app.state_path))
    };
    let mut footer = vec![Line::raw(location)];
    footer.extend(
        app.notes
            .iter()
            .map(|note| Line::raw(format!(" {}", safe(note)))),
    );
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), rows[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_displays_metadata_storage_and_colored_selection() {
        let mut ui = Ui::new(1).unwrap();
        ui.loading = false;
        ui.page = std::sync::Arc::new(vec![Entry {
            id: 2,
            name: "photo.jpg".into(),
            size: 4096,
            folder: false,
            modified: 1,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        }]);
        let mut state = Snapshot {
            connection: Connection::Ready,
            storage: Some(Storage {
                used: 1024,
                total: 4096,
            }),
            ..Snapshot::default()
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 28)).unwrap();
        terminal.draw(|f| draw(f, &ui, &state)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for label in ["photo.jpg", "4.0 KiB", "Modified", "Connected", "25% used"] {
            assert!(rendered.contains(label), "Missing {label}");
        }
        assert!(buffer.content.iter().any(|c| c.fg == ACCENT));
        assert!(
            buffer
                .content
                .iter()
                .any(|c| c.symbol() == "●" && c.fg == SUCCESS)
        );
        let header = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        assert!(header.contains("megawren   /"));
        assert!(rendered.contains("1 Cloud drive"));
        assert!(rendered.contains("2 Rubbish Bin"));
        assert!(!header.contains("/ /"));
        state.jobs.push_back(JobView {
            items: std::sync::Arc::from([]),
            current_item: None,
            id: 1,
            direction: Direction::Download,
            name: "movie.mkv".into(),
            file: "movie.mkv".into(),
            destination: "/synthetic".into(),
            state: JobState::Transferring,
            bytes: 512,
            total: 1024,
            speed: 4096.0,
            files: 0,
            folders: 0,
            error: None,
        });
        terminal.draw(|f| draw(f, &ui, &state)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("4.0 KiB/s"));
        assert!(!text.contains("0 complete") && !text.contains("0 files"));
        state.jobs[0].files = 2;
        terminal.draw(|f| draw(f, &ui, &state)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("2 files saved"));
    }
    #[test]
    fn download_picker_explains_its_destination_and_action() {
        let mut ui = Ui::new(1).unwrap();
        ui.loading = false;
        ui.page = std::sync::Arc::new(vec![Entry {
            id: 2,
            name: "backup.tar.zst".into(),
            size: 42,
            folder: false,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        }]);
        ui.open_download();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 28)).unwrap();
        terminal
            .draw(|f| draw(f, &ui, &Snapshot::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for label in [
            "Choose download folder",
            "Download: backup.tar.zst",
            "Save in:",
            "Folders on this computer",
            "Parent folder",
            "Ctrl+L Location",
            "Output:",
            "d  Download to this folder",
        ] {
            assert!(text.contains(label), "Missing {label}");
        }
    }
}
