use super::*;
use image::Rgba;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use ratatui_image::{
    StatefulImage,
    protocol::{
        StatefulProtocol, StatefulProtocolType, halfblocks::Halfblocks, iterm2::Iterm2,
        kitty::StatefulKitty,
    },
};
use std::{
    cell::RefCell,
    io::{IsTerminal, Write},
};

pub(super) struct PreviewImage {
    protocol: StatefulProtocol,
    kitty: Option<u32>,
    font: (u16, u16),
}
impl Drop for PreviewImage {
    fn drop(&mut self) {
        if let Some(id) = self.kitty
            && io::stdout().is_terminal()
        {
            let _ = write!(io::stdout(), "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
            let _ = io::stdout().flush();
        }
    }
}
fn image_protocol(image: image::DynamicImage) -> PreviewImage {
    let window = crossterm::terminal::window_size().ok();
    let cell = window.and_then(|w| {
        if w.columns > 0 && w.rows > 0 && w.width / w.columns > 0 && w.height / w.rows > 0 {
            Some((w.width / w.columns, w.height / w.rows))
        } else {
            None
        }
    });
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let multiplexer = term.starts_with("tmux") || term.starts_with("screen") || program == "tmux";
    let mut kitty = None;
    let protocol = if cell.is_some()
        && !multiplexer
        && (term.contains("kitty") || program == "ghostty")
    {
        let mut bytes = [0u8; 4];
        if getrandom::fill(&mut bytes).is_ok() {
            let id = u32::from_ne_bytes(bytes).max(1);
            kitty = Some(id);
            StatefulProtocolType::Kitty(StatefulKitty::new(id, false, false))
        } else {
            StatefulProtocolType::Halfblocks(Halfblocks::default())
        }
    } else if cell.is_some() && !multiplexer && (program.contains("iTerm") || program == "WezTerm")
    {
        StatefulProtocolType::ITerm2(Iterm2::default())
    } else {
        StatefulProtocolType::Halfblocks(Halfblocks::default())
    };
    let font = cell.unwrap_or((8, 16));
    PreviewImage {
        protocol: StatefulProtocol::new(image, font.into(), Some(Rgba([0, 0, 0, 0])), protocol),
        kitty,
        font,
    }
}
impl Ui {
    pub(super) fn open_preview(&mut self, commands: &SyncSender<Command>) {
        if let Some(entry) = self.page.get(self.cursor).cloned() {
            self.preview_entry(entry, commands);
        }
    }
    pub(super) fn preview_entry(&mut self, entry: Entry, commands: &SyncSender<Command>) {
        if let Err(error) = crate::preview::plan(&entry.name, entry.size, entry.folder) {
            self.notice = Some((
                if entry.folder {
                    "Enter opens this folder. Preview is available for files.".into()
                } else {
                    error.to_string()
                },
                std::time::Instant::now(),
            ));
            return;
        }
        let request = self.serial();
        if self.send(commands, Command::Preview { request, entry }) {
            self.modal = Some(Modal::Preview {
                request,
                scroll: 0,
                column: 0,
                image: RefCell::new(None),
            });
        }
    }
}
fn centered_image(image: &PreviewImage, area: Rect) -> Rect {
    // Bound the raster on oversized terminals while preserving the cell aspect ratio.
    let font = image.font;
    let bounded = Rect::new(
        area.x,
        area.y,
        area.width.min(2048 / font.0.max(1)),
        area.height.min(2048 / font.1.max(1)),
    );
    let size = image
        .protocol
        .size_for(ratatui_image::Resize::Scale(None), bounded.as_size());
    let width = size.width.min(area.width);
    let height = size.height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
pub(super) fn draw(
    frame: &mut Frame<'_>,
    rows: &std::cell::Cell<u16>,
    state: &Snapshot,
    request: u64,
    scroll: u16,
    column: u16,
    image: &RefCell<Option<PreviewImage>>,
) {
    let screen = frame.area();
    let area = Rect::new(
        screen.x + 1,
        screen.y + 1,
        screen.width.saturating_sub(2),
        screen.height.saturating_sub(2),
    );
    frame.render_widget(Clear, area);
    let preview = state.preview.as_ref().filter(|p| p.request == request);
    let title = preview
        .filter(|p| !p.name.is_empty())
        .map(|p| format!(" Preview · {} ", safe(&p.name)))
        .unwrap_or(" Preview ".into());
    let block = Block::default()
        .title(Line::styled(
            title,
            Style::default()
                .fg(Color::Indexed(81))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Indexed(240)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    rows.set(parts[0].height);
    let mut hint = "Esc close".to_string();
    if let Some(preview) = preview {
        if let Some(error) = &preview.error {
            frame.render_widget(
                Paragraph::new(safe(error)).style(Style::default().fg(Color::Indexed(203))),
                parts[0],
            );
        } else if let Some(content) = &preview.content {
            match content.as_ref() {
                crate::preview::Content::Text { lines, truncated } => {
                    let rows: Vec<_> = lines
                        .iter()
                        .enumerate()
                        .skip(scroll as usize)
                        .take(parts[0].height as usize)
                        .map(|(i, line)| {
                            Line::from(vec![
                                Span::styled(
                                    format!("{:>4}  ", i + 1),
                                    Style::default().fg(Color::Indexed(245)),
                                ),
                                Span::raw(line.clone()),
                            ])
                        })
                        .collect();
                    if lines.is_empty() {
                        frame.render_widget(Paragraph::new("This file is empty."), parts[0]);
                    } else {
                        frame.render_widget(Paragraph::new(rows).scroll((0, column)), parts[0]);
                    }
                    hint = format!(
                        "↑↓ scroll · ←→ pan · Esc close{}",
                        if *truncated {
                            " · Partial preview"
                        } else {
                            ""
                        }
                    );
                }
                crate::preview::Content::Image(data) => {
                    let mut image = image.borrow_mut();
                    if image.is_none() {
                        *image = Some(image_protocol(data.clone()));
                    }
                    let current = image.as_mut().unwrap();
                    frame.render_stateful_widget(
                        StatefulImage::default().resize(ratatui_image::Resize::Scale(None)),
                        centered_image(current, parts[0]),
                        &mut current.protocol,
                    );
                    if current
                        .protocol
                        .last_encoding_result()
                        .is_some_and(|result| result.is_err())
                    {
                        *current = PreviewImage {
                            kitty: None,
                            font: (8, 16),
                            protocol: StatefulProtocol::new(
                                data.clone(),
                                (8, 16).into(),
                                Some(Rgba([0, 0, 0, 0])),
                                StatefulProtocolType::Halfblocks(Halfblocks::default()),
                            ),
                        };
                        frame.render_stateful_widget(
                            StatefulImage::default().resize(ratatui_image::Resize::Scale(None)),
                            centered_image(current, parts[0]),
                            &mut current.protocol,
                        );
                    }
                }
            }
        } else {
            frame.render_widget(
                Paragraph::new(if preview.decoding {
                    "Opening preview…".into()
                } else {
                    format!(
                        "Loading preview… {}% · {}",
                        percent(preview.bytes, preview.total),
                        size(preview.total)
                    )
                })
                .style(Style::default().fg(Color::Indexed(81))),
                parts[0],
            );
            hint = "Esc cancel preview · Browsing does not fetch previews".into();
        }
    } else {
        frame.render_widget(Paragraph::new("Preparing preview…"), parts[0]);
    }
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::Indexed(81))),
        parts[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_previews_fill_the_limiting_axis_and_stay_centered() {
        for (width, height) in [(320, 160), (16, 64), (8, 8)] {
            let image = image_protocol(image::DynamicImage::new_rgb8(width, height));
            for area in [Rect::new(2, 3, 96, 22), Rect::new(2, 3, 70, 14)] {
                let target = centered_image(&image, area);
                assert!(
                    target.width >= area.width - 1 || target.height >= area.height - 1,
                    "{target:?}"
                );
                assert!(target.width <= area.width && target.height <= area.height);
                assert!((target.x - area.x).abs_diff(area.right() - target.right()) <= 1);
                assert!((target.y - area.y).abs_diff(area.bottom() - target.bottom()) <= 1);
            }
        }
    }
}
