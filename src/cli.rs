//! Shell operands are validated before starting the SDK. The transfer worker
//! reopens them with its normal filesystem boundaries at the point of use.
use crate::{Command, input, prompt, state, transfer::Boundary};
use anyhow::{Result, ensure};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use zeroize::Zeroizing;

pub enum Transfer {
    Get {
        sources: Vec<Zeroizing<String>>,
        destination: PathBuf,
        overwrite: bool,
    },
    Put {
        sources: Vec<PathBuf>,
        destination: String,
    },
}

pub fn remote_path(path: &str) -> Result<()> {
    ensure!(path.starts_with('/'), "Remote paths must begin with /");
    path.split('/')
        .filter(|p| !p.is_empty())
        .try_for_each(state::component)
}

fn destination(paths: &mut Vec<PathBuf>, default: &str) -> PathBuf {
    if paths.len() > 1 {
        paths.pop().unwrap()
    } else {
        PathBuf::from(default)
    }
}

impl Transfer {
    pub fn prepare(
        command: Option<&mut Command>,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>> {
        match command {
            Some(Command::Get { paths, overwrite }) => {
                let mut paths = std::mem::take(paths);
                let mut to = destination(&mut paths, ".");
                let sources = if paths.is_empty() {
                    ensure!(
                        input::can_prompt(),
                        "Supply a remote path or MEGA link to download."
                    );
                    let source = prompt("Remote path or MEGA link: ", cancel)?;
                    let folder = crate::line("Download into folder [.]: ", cancel)?;
                    if !folder.is_empty() {
                        to = PathBuf::from(folder);
                    }
                    vec![source]
                } else {
                    paths
                        .into_iter()
                        .map(|p| {
                            p.into_os_string()
                                .into_string()
                                .map(Zeroizing::new)
                                .map_err(|_| anyhow::anyhow!("Remote paths must be UTF-8"))
                        })
                        .collect::<Result<Vec<_>>>()?
                };
                for source in &sources {
                    if source.starts_with("https://mega.nz/") {
                        ensure!(
                            sources.len() == 1,
                            "Download a public link in its own command."
                        );
                        ensure!(
                            !source.contains(['\r', '\n', '\0'])
                                && (source.starts_with("https://mega.nz/file/")
                                    || source.starts_with("https://mega.nz/folder/")),
                            "Invalid MEGA link"
                        );
                    } else {
                        remote_path(source)?;
                    }
                }
                ensure!(
                    !to.as_os_str()
                        .as_encoded_bytes()
                        .starts_with(b"https://mega.nz/"),
                    "The last argument must be a local directory. Download each public link in its own command."
                );
                Boundary::new(Some(root))?.directory(&to)?;
                Ok(Some(Self::Get {
                    sources,
                    destination: to,
                    overwrite: *overwrite,
                }))
            }
            Some(Command::Put { paths }) => {
                let mut sources = std::mem::take(paths);
                let to = destination(&mut sources, "/")
                    .into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Remote paths must be UTF-8"))?;
                remote_path(&to)?;
                let boundary = Boundary::new(Some(root))?;
                let mut names = HashSet::new();
                for path in &sources {
                    input::check(cancel)?;
                    let source = boundary
                        .source(path)
                        .map_err(|e| anyhow::anyhow!("Cannot upload {}: {e}", path.display()))?;
                    ensure!(
                        names.insert(source.name),
                        "Upload sources have the same destination name"
                    );
                }
                Ok(Some(Self::Put {
                    sources,
                    destination: to,
                }))
            }
            Some(Command::Ls { remote, .. }) => {
                remote_path(remote)?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
    pub fn public_link(&self) -> Option<&str> {
        match self {
            Self::Get { sources, .. } if sources[0].starts_with("https://mega.nz/") => {
                Some(&sources[0])
            }
            _ => None,
        }
    }
}
