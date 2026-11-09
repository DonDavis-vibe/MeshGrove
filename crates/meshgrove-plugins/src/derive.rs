//! Files of a bundle the host makes itself: a copy of a 3MF in which the plugin swapped the
//! meshes of some parts ([`meshgrove_plugin_api::Derive3mf`]). Done when the bundle is handed
//! over, in its staging folder; from then on the copy is a file of the bundle like any other.

use std::path::{Path, PathBuf};

use meshgrove_core::loaders::threemf::edit::{Replacement, replace_parts};
use meshgrove_core::{Kind, Mesh, archive, loaders};
use meshgrove_plugin_api::{Bundle, Derive3mf};

use crate::Error;
use crate::guest_files::{self, GuestDirs, file_name, plain_parts, read_plain};
use crate::library::{Reading, split_part};

/// Makes every file of `bundle` that is derived from a 3MF, below `out` (the staging folder).
/// The source is read as `reading` may read the library, or from the plugin's own folders
/// (`dirs`). The meshes the plugin wrote for it are used up: they are removed from `out`.
pub(crate) fn derive_files(bundle: &Bundle, out: &Path, reading: &Reading, dirs: &GuestDirs) -> Result<(), Error> {
    let mut used: Vec<PathBuf> = Vec::new();
    for file in &bundle.files {
        let Some(derive) = &file.derive_3mf else { continue };
        let to = below(out, &file.path)?;
        if to.symlink_metadata().is_ok() {
            return Err(Error::Invalid(format!(
                "{:?} is there already: a derived file is written by Meshgrove",
                file.path
            )));
        }
        let made = derive_one(derive, out, &to, reading, dirs, &mut used);
        if made.is_err() {
            let _ = std::fs::remove_file(&to);
        }
        made.map_err(|e| match e {
            Error::Failed(why) => Error::Failed(format!("{}: {why}", file.path)),
            other => other,
        })?;
    }
    // What is left of the folders the meshes were in, if nothing else is in them.
    for mesh in &used {
        let _ = std::fs::remove_file(mesh);
        for dir in mesh.ancestors().skip(1).take_while(|dir| dir.starts_with(out) && *dir != out) {
            if std::fs::remove_dir(dir).is_err() {
                break;
            }
        }
    }
    Ok(())
}

/// The place of `path` (as a bundle names files) below `out`.
fn below(out: &Path, path: &str) -> Result<PathBuf, Error> {
    let parts = plain_parts(path).ok_or_else(|| Error::Invalid(format!("{path:?} is not a plain path below /out")))?;
    Ok(parts.iter().fold(out.to_path_buf(), |p, part| p.join(part)))
}

fn derive_one(
    derive: &Derive3mf,
    out: &Path,
    to: &Path,
    reading: &Reading,
    dirs: &GuestDirs,
    used: &mut Vec<PathBuf>,
) -> Result<(), Error> {
    let source = derive.source.trim();
    if !matches!(split_part(source), Ok((_, None))) {
        return Err(Error::Invalid("the source of a derived 3MF is a file, not a part".into()));
    }
    let name = match guest_files::is_guest_path(source) {
        true => file_name(source).to_owned(),
        false => source.rsplit('/').next().unwrap_or(source).to_owned(),
    };
    if Kind::from_file_name(&name) != Kind::ThreeMf {
        return Err(Error::Invalid(format!("{name} is not a 3MF: only those have parts to replace")));
    }
    let bytes = if guest_files::is_guest_path(source) {
        dirs.read(source).map_err(Error::Invalid)?
    } else {
        let found = reading.locate(source).map_err(Error::Denied)?;
        archive::read(&found.path).map_err(|e| Error::Failed(format!("{name}: {e}")))?
    };
    let mut meshes: Vec<(usize, Mesh)> = Vec::new();
    for swap in &derive.replace {
        let part = match split_part(&swap.part) {
            Ok((of, Some(part))) if of.trim() == source => part,
            _ => {
                return Err(Error::Invalid(format!(
                    "a part to replace is not one of {name} (tokens come from describe)"
                )));
            }
        };
        let parts = plain_parts(&swap.mesh)
            .ok_or_else(|| Error::Invalid(format!("{:?} is not a plain path below /out", swap.mesh)))?;
        let shown = format!("/out/{}", swap.mesh);
        let data = read_plain(out, &parts, &shown).map_err(Error::Invalid)?;
        // The plugin writes millimetres, whatever unit the user reads their own files in.
        let mesh = loaders::load_mesh_mm(Kind::from_file_name(file_name(&swap.mesh)), &data)
            .map_err(|e| Error::Failed(format!("{shown}: {e}")))?;
        meshes.push((part, mesh));
        let path = parts.iter().fold(out.to_path_buf(), |p, part| p.join(part));
        if !used.contains(&path) {
            used.push(path);
        }
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let replacements: Vec<Replacement> = meshes.iter().map(|(part, mesh)| Replacement { part: *part, mesh }).collect();
    let mut file = std::io::BufWriter::new(std::fs::File::create_new(to)?);
    replace_parts(&bytes, &replacements, &mut file).map_err(|e| Error::Failed(e.to_string()))?;
    std::io::Write::flush(&mut file)?;
    Ok(())
}
