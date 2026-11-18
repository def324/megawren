use super::*;
use crate::preview::{self, Content, Plan};
use std::{fs::File, sync::Arc, thread::JoinHandle};

pub(super) struct Pending {
    request: u64,
    entry: Entry,
    plan: Plan,
    file: Option<File>,
    decode: Option<JoinHandle<Result<Content>>>,
    pub cancel: Arc<AtomicBool>,
    published: Instant,
}
impl<R: Remote> Engine<'_, R> {
    pub(super) fn preview_error(&self, request: u64, error: String) {
        let name = snapshot(&self.shared)
            .preview
            .as_ref()
            .filter(|p| p.request == request)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        update(&self.shared, |s| {
            s.preview = Some(Arc::new(PreviewState {
                request,
                name,
                bytes: 0,
                total: 0,
                decoding: false,
                content: None,
                error: Some(safe(&error)),
            }))
        });
    }
    pub(super) fn start_preview(&mut self, request: u64, entry: Entry) -> Result<()> {
        let plan = preview::plan(&entry.name, entry.size, entry.folder)?;
        ensure!(
            self.preview.is_none(),
            "Previous preview is stopping; try again shortly"
        );
        let current = self.remote.info(entry.id)?;
        ensure!(
            current.name == entry.name
                && current.size == entry.size
                && current.modified == entry.modified,
            "File changed; select it again"
        );
        let file = self.remote.preview_file()?;
        if plan.bytes > 0 {
            self.remote.preview(entry.id, &file, plan.bytes)?;
        }
        update(&self.shared, |s| {
            s.preview = Some(Arc::new(PreviewState {
                request,
                name: entry.name.clone(),
                bytes: 0,
                total: plan.bytes,
                decoding: false,
                content: None,
                error: None,
            }))
        });
        self.preview = Some(Pending {
            request,
            entry,
            plan,
            file: Some(file),
            decode: None,
            cancel: Arc::new(AtomicBool::new(false)),
            published: Instant::now() - Duration::from_secs(1),
        });
        Ok(())
    }
    pub(super) fn close_preview(&mut self, request: u64) {
        if let Some(pending) = self.preview.as_ref().filter(|p| p.request == request) {
            pending.cancel.store(true, Ordering::Relaxed);
            let _ = self.remote.cancel_preview();
        }
        if snapshot(&self.shared)
            .preview
            .as_ref()
            .is_some_and(|p| p.request == request)
        {
            update(&self.shared, |s| s.preview = None);
        }
    }
    pub(super) fn preview_tick(&mut self) {
        let Some(mut pending) = self.preview.take() else {
            return;
        };
        if let Some(handle) = pending.decode.take() {
            if !handle.is_finished() {
                pending.decode = Some(handle);
                self.preview = Some(pending);
                return;
            }
            let result = handle
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Image decoder could not finish")));
            if !pending.cancel.load(Ordering::Relaxed) {
                match result {
                    Ok(content) => update(&self.shared, |s| {
                        s.preview = Some(Arc::new(PreviewState {
                            request: pending.request,
                            name: pending.entry.name,
                            bytes: pending.plan.bytes,
                            total: pending.plan.bytes,
                            decoding: false,
                            content: Some(Arc::new(content)),
                            error: None,
                        }))
                    }),
                    Err(error) => self.preview_error(pending.request, error.to_string()),
                }
            }
            return;
        }
        let event = if pending.plan.bytes == 0 {
            Ok(ffi::Event {
                done: true,
                code: 0,
                bytes: 0,
                total: 0,
                retry: 0,
            })
        } else {
            self.remote.preview_event()
        };
        match event {
            Ok(event) if !event.done => {
                if !pending.cancel.load(Ordering::Relaxed)
                    && pending.published.elapsed() >= Duration::from_millis(200)
                {
                    update(&self.shared, |s| {
                        s.preview = Some(Arc::new(PreviewState {
                            request: pending.request,
                            name: pending.entry.name.clone(),
                            bytes: event.bytes,
                            total: pending.plan.bytes,
                            decoding: false,
                            content: None,
                            error: None,
                        }))
                    });
                    pending.published = Instant::now();
                }
                self.preview = Some(pending);
            }
            Ok(_) if pending.cancel.load(Ordering::Relaxed) => (),
            Ok(event) if event.code == 0 => {
                let file = pending.file.take().unwrap();
                let plan = pending.plan;
                let cancel = pending.cancel.clone();
                let handle = std::thread::Builder::new()
                    .name("preview".into())
                    .spawn(move || preview::decode(file, plan, cancel));
                match handle {
                    Ok(handle) => {
                        pending.decode = Some(handle);
                        update(&self.shared, |s| {
                            if let Some(p) = s.preview.as_mut() {
                                Arc::make_mut(p).decoding = true;
                            }
                        });
                        self.preview = Some(pending);
                    }
                    Err(error) => self.preview_error(pending.request, error.to_string()),
                }
            }
            Ok(event) => self.preview_error(
                pending.request,
                format!("Preview download failed ({})", event.code),
            ),
            Err(error) => {
                let _ = self.remote.cancel_preview();
                self.preview_error(pending.request, error.to_string());
            }
        }
    }
}
