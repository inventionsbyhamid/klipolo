//! A project's `contrib/` folder: the copies a project
//! carries of the contributions it uses, `contrib/<plugin>/<local>/`. A project can come from anywhere, so the folder
//! is read through folder handles (`handle`: each level opened from the one above it, never following a link, never
//! looking a path up again after it was checked), each version's files within their caps and every file through
//! `load_files`' checks. At most `PackLimits::MAX_CONTRIBS` contributions and `MAX_EMBEDDED_BYTES` in all; a symlinked
//! `contrib/` is refused, and its listing stops after `MAX_CONTRIBS` entries. A folder that doesn't load is named with
//! why, and the project still opens: its effect draws as nothing.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

use klipolo_model::contrib::ContribId;

use super::read::{MAX_LISTED, io, os_litter};
use super::{LoadedContrib, PackError, PackLimits, load_files, quoted};

mod handle;
use handle::{Dir, NotPlain};

/// What a project's copies may hold in all, files of every version together.
pub const MAX_EMBEDDED_BYTES: u64 = 4 << 20;

/// A folder that didn't load, and why: `folder` names it (`plugin/local`, quoted and escaped); `id` is the version's
/// when it was a version folder (the app replaces its own versions' copies that don't read).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub folder: String,
    pub id: Option<ContribId>,
    pub why: PackError,
}

/// What `contrib/` holds: the versions that loaded, the folders that didn't, how many version folders were examined
/// and how many bytes were read. The app's commit path keeps within the same caps (`examined` within
/// `PackLimits::MAX_CONTRIBS`, `bytes` within `MAX_EMBEDDED_BYTES`), so what it writes is what this reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Embedded {
    pub loaded: Vec<LoadedContrib>,
    pub refused: Vec<Refused>,
    pub examined: usize,
    pub bytes: u64,
}

/// The project's `contrib` folder read (see the module's doc). A missing folder holds nothing.
pub fn read_embedded_dir(contrib: &Path, limits: &PackLimits) -> Embedded {
    let mut out = Embedded::default();
    let root = match Dir::open(contrib) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.refuse("contrib", refusal("contrib", &e));
            return out;
        }
    };
    let max = PackLimits::MAX_CONTRIBS;
    let plugins = match listing(&root, max) {
        Ok((names, more)) => {
            if more {
                out.refuse("contrib", too_many("contrib", max));
            }
            names
        }
        Err(e) => {
            out.refuse("contrib", io("contrib", &e));
            return out;
        }
    };
    for plugin in plugins {
        let plugin = match plugin {
            Ok(name) => name,
            Err(why) => {
                out.refuse("contrib", why);
                continue;
            }
        };
        let dir = match root.sub(&plugin) {
            Ok(dir) => dir,
            Err(e) => {
                out.refuse(&plugin, refusal(&plugin, &e));
                continue;
            }
        };
        let locals = match listing(&dir, max) {
            Ok((names, more)) => {
                if more {
                    out.refuse(&plugin, too_many(&plugin, max));
                }
                names
            }
            Err(e) => {
                out.refuse(&plugin, io(&plugin, &e));
                continue;
            }
        };
        for local in locals {
            let local = match local {
                Ok(name) => name,
                Err(why) => {
                    out.refuse(&plugin, why);
                    continue;
                }
            };
            let name = format!("{plugin}/{local}");
            if out.examined == max {
                out.refuse(&name, too_many("contrib", max));
                return out;
            }
            out.examined += 1;
            let id = ContribId::new(&plugin, &local);
            let read = version(&dir, &local, &id, limits, &mut out.bytes)
                .and_then(|files| load_files(&id, files, contrib.join(&plugin).join(&local)));
            match read {
                Ok(loaded) => out.loaded.push(loaded),
                Err(why) => out.refused.push(Refused {
                    folder: quoted(&name),
                    id: Some(id),
                    why,
                }),
            }
        }
    }
    out
}

impl Embedded {
    fn refuse(&mut self, folder: &str, why: PackError) {
        self.refused.push(Refused {
            folder: quoted(folder),
            id: None,
            why,
        });
    }
}

/// The names in `dir`, the operating system's litter and copies being written left out: the first `max`, and whether
/// it holds more (the listing stops there, m19). A name that isn't Unicode comes back refused on its own.
fn listing(dir: &Dir, max: usize) -> std::io::Result<(Vec<Result<String, PackError>>, bool)> {
    let (names, more) = dir.names(max)?;
    let names = names
        .into_iter()
        .filter_map(|name: OsString| match name.into_string() {
            // A copy being written (`.tmp-<local>`, the app's) is passed over until it is renamed into place.
            Ok(name) if os_litter(&name) || name.starts_with(".tmp-") => None,
            Ok(name) => Some(Ok(name)),
            Err(name) => Some(Err(PackError::Extra(quoted(&name.to_string_lossy())))),
        })
        .collect();
    Ok((names, more))
}

/// What a folder holding more than `max` entries is refused with, for the rest.
fn too_many(what: &str, max: usize) -> PackError {
    PackError::Extra(format!("{} holds more than {max} entries", quoted(what)))
}

/// Version folder `local` under `parent`, its files each within its cap, adding to `total` (at most
/// `MAX_EMBEDDED_BYTES` across the project's copies).
fn version(
    parent: &Dir,
    local: &str,
    id: &ContribId,
    limits: &PackLimits,
    total: &mut u64,
) -> Result<BTreeMap<String, Vec<u8>>, PackError> {
    if !id.is_well_formed() {
        return Err(PackError::BadId(quoted(id.as_str())));
    }
    let dir = parent.sub(local).map_err(|e| refusal(local, &e))?;
    let (names, more) = listing(&dir, MAX_LISTED).map_err(|e| io(local, &e))?;
    if more {
        return Err(too_many(local, MAX_LISTED));
    }
    let mut files = BTreeMap::new();
    for name in names {
        let name = name?;
        let Some(limit) = limits.of(&name) else {
            return Err(PackError::Extra(quoted(&name)));
        };
        let file = dir.file(&name).map_err(|e| refusal(&name, &e))?;
        let meta = file.metadata().map_err(|e| io(&name, &e))?;
        if !meta.is_file() {
            return Err(PackError::Link(quoted(&name)));
        }
        let too_large = |file: &str, limit| PackError::TooLarge {
            file: quoted(file),
            limit,
        };
        if meta.len() > limit {
            return Err(too_large(&name, limit));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io(&name, &e))?;
        if bytes.len() as u64 > limit {
            return Err(too_large(&name, limit));
        }
        *total += bytes.len() as u64;
        if *total > MAX_EMBEDDED_BYTES {
            return Err(too_large("contrib", MAX_EMBEDDED_BYTES));
        }
        files.insert(name, bytes);
    }
    Ok(files)
}

/// A link (or anything but a plain folder or file) as `Link`, anything else as `Io`, naming `what`.
fn refusal(what: &str, e: &std::io::Error) -> PackError {
    if e.get_ref().is_some_and(|inner| inner.is::<NotPlain>()) {
        PackError::Link(quoted(what))
    } else {
        io(what, e)
    }
}
