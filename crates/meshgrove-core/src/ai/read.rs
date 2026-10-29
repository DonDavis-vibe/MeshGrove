//! The reading tools: what an AI may learn about the library, always within the context's
//! [`super::Scope`]. Nothing here writes.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use rusqlite::{OptionalExtension, params, params_from_iter};

use super::scope::below_sql;
use super::types::*;
use super::vocab::Vocabulary;
use super::{Context, ToolError, ToolResult, untrusted};
use crate::checks::{self, CHECKS_VERSION, MeshChecks};
use crate::db::{Error, Library, entry_from_row};
use crate::formats::Kind;
use crate::inbox::INBOX_KINDS;
use crate::listing::{self, Entry};
use crate::lists::{ListKind, Marks};
use crate::printer::{self, Fit};
use crate::query::{self, Mark};

/// Most characters of a readme or note handed out.
const TEXT_CHARS: usize = 2000;
const DEFAULT_PAGE: usize = 50;
const MAX_PAGE: usize = 200;
/// Most pictures per `get_thumbnails` call.
pub const MAX_THUMBNAILS: usize = 12;
/// Most files per `check_printability` call.
pub const MAX_CHECKS: usize = 100;

/// A file with the facts the index holds about it.
pub(crate) struct Row {
    pub entry: Entry,
    pub hash: Option<String>,
    pub size: Option<[f32; 3]>,
    pub triangles: Option<u64>,
    pub print_s: Option<u32>,
    pub gcode_printer: Option<String>,
    pub filament_g: Option<f32>,
    pub meta_key: Option<String>,
}

const ROW_COLUMNS: &str = "d.path, f.name, f.kind, f.size, f.mtime_ns, f.content_hash, f.size_x, f.size_y, f.size_z, \
                           f.triangles, f.print_s, f.gcode_printer, f.filament_g, f.meta_key";

fn row_from(r: &rusqlite::Row) -> Result<Row, Error> {
    let size = match (r.get::<_, Option<f64>>(6)?, r.get::<_, Option<f64>>(7)?, r.get::<_, Option<f64>>(8)?) {
        (Some(x), Some(y), Some(z)) => Some([x as f32, y as f32, z as f32]),
        _ => None,
    };
    Ok(Row {
        entry: entry_from_row(r)?,
        hash: r.get(5)?,
        size,
        triangles: r.get::<_, Option<i64>>(9)?.map(|t| t as u64),
        print_s: r.get::<_, Option<i64>>(10)?.map(|s| s as u32),
        gcode_printer: r.get(11)?,
        filament_g: r.get::<_, Option<f64>>(12)?.map(|g| g as f32),
        meta_key: r.get(13)?,
    })
}

impl Library {
    pub(crate) fn ai_rows(
        &self,
        conditions: &str,
        args: &[String],
        order: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Row>, Error> {
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM files f JOIN dirs d ON d.id = f.dir_id WHERE 1 {conditions}
             ORDER BY {order} LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(args), row_from)?.collect()
    }

    pub(crate) fn ai_count(&self, conditions: &str, args: &[String]) -> Result<usize, Error> {
        let sql = format!("SELECT COUNT(*) FROM files f JOIN dirs d ON d.id = f.dir_id WHERE 1 {conditions}");
        Ok(self.conn.query_row(&sql, params_from_iter(args), |r| r.get::<_, i64>(0))? as usize)
    }

    /// The row of one file, also an archive member (stored with its inner path as name).
    pub(crate) fn ai_row_of(&self, path: &Path) -> Result<Option<Row>, Error> {
        let mut candidates = Vec::new();
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            candidates.push((dir.to_path_buf(), name.to_string_lossy().into_owned()));
        }
        if let Some((archive, member)) = crate::archive::split(path) {
            candidates.push((archive, member));
        }
        for (dir, name) in candidates {
            let row = self
                .conn
                .query_row(
                    &format!(
                        "SELECT {ROW_COLUMNS} FROM files f JOIN dirs d ON d.id = f.dir_id WHERE d.path = ?1 AND f.name = ?2"
                    ),
                    params![dir.to_string_lossy(), name],
                    row_from,
                )
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
        Ok(None)
    }

    fn checks_by_hash(&self, hash: &str) -> Result<Option<MeshChecks>, Error> {
        let facts: Option<String> = self
            .conn
            .query_row(
                "SELECT facts FROM mesh_checks WHERE content_hash = ?1 AND version = ?2",
                params![hash, CHECKS_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        Ok(facts.and_then(|f| serde_json::from_str(&f).ok()))
    }

    fn open_inbox(&self) -> Result<HashSet<String>, Error> {
        let mut stmt = self.conn.prepare("SELECT content_hash FROM inbox WHERE done_ns IS NULL")?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }

    fn paths_with(&self, hash: &str) -> Result<Vec<PathBuf>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.content_hash = ?1 ORDER BY d.path, f.name",
        )?;
        stmt.query_map([hash], |r| entry_from_row(r).map(|e| e.path))?.collect()
    }
}

/// What summaries need beyond a row; read once per tool call.
struct Facts {
    marks: HashMap<PathBuf, Marks>,
    new: HashSet<String>,
}

impl Facts {
    fn load(lib: &Library) -> ToolResult<Facts> {
        Ok(Facts { marks: lib.marks()?, new: lib.open_inbox()? })
    }
}

fn push(args: &mut Vec<String>, value: impl Into<String>) -> String {
    args.push(value.into());
    format!("?{}", args.len())
}

fn fit_out(printer: &str, fit: Fit) -> PrinterFit {
    let (fit, rotate_deg, over_mm) = match fit {
        Fit::Fits => ("fits", None, None),
        Fit::Rotated(a) => ("rotated", Some(a), None),
        Fit::TooBig(over) => ("too_big", None, Some(over)),
    };
    PrinterFit { printer: printer.to_owned(), fit: fit.to_owned(), rotate_deg, over_mm }
}

fn summary(cx: &Context, row: &Row, facts: &Facts) -> ModelSummary {
    let path = &row.entry.path;
    let kind = row.entry.kind;
    let marks = facts.marks.get(path);
    let checks = row.hash.as_deref().and_then(|h| cx.lib.checks_by_hash(h).ok().flatten());
    let (mut fits, mut too_big_for) = (Vec::new(), Vec::new());
    if let Some(size) = row.size.filter(|_| kind.is_model()) {
        for (p, fit) in printer::fits_all(size, &cx.printers) {
            match fit {
                Fit::TooBig(_) => too_big_for.push(p.name.clone()),
                _ => fits.push(p.name.clone()),
            }
        }
    }
    let problems = checks::problems(kind, row.size, checks.as_ref(), &cx.printers, cx.config.model_units)
        .iter()
        .map(|p| p.code().to_owned())
        .collect();
    let gcode = (kind == Kind::Gcode).then(|| GcodeSummary {
        sliced_for: row.gcode_printer.clone(),
        printer: row
            .gcode_printer
            .as_deref()
            .and_then(|g| printer::printer_for_gcode(g, &cx.printers))
            .map(|p| p.name.clone()),
        print_minutes: row.print_s.map(|s| s.div_ceil(60)),
        filament_g: row.filament_g,
    });
    ModelSummary {
        id: cx.id_of(path),
        name: listing::file_name(path),
        kind: kind.as_str().to_owned(),
        folder: path.parent().map(|p| cx.id_of(p)).unwrap_or_default(),
        file_size: row.entry.size,
        size_mm: row.size,
        triangles: row.triangles,
        tags: marks.map(|m| m.tags.clone()).unwrap_or_default(),
        new: row.hash.as_ref().is_some_and(|h| facts.new.contains(h)),
        favorite: marks.is_some_and(|m| m.favorite),
        printed: marks.map_or(0, |m| m.printed),
        last_print_failed: marks.is_some_and(|m| m.last_failed),
        fits,
        too_big_for,
        problems,
        gcode,
    }
}

/// The SQL conditions of a filter, always within the scope.
fn filter_sql(cx: &Context, f: &ModelFilter, args: &mut Vec<String>) -> ToolResult<String> {
    let mut q = f.q.as_deref().map(query::parse).unwrap_or_default();
    q.tags.extend(f.tags_all.iter().map(|t| t.trim().to_lowercase()));
    for k in &f.kinds {
        match Kind::parse(&k.trim().to_lowercase()) {
            Kind::Other | Kind::Folder => return Err(ToolError::Invalid(format!("unknown kind {k:?}"))),
            kind => q.kinds.push(kind),
        }
    }
    if let Some(list) = &f.list {
        q.lists.push(list.trim().to_lowercase());
    }
    let mut fits_any = false;
    match f.fits_printer.as_deref().map(str::trim) {
        Some(p) if p.eq_ignore_ascii_case("any") => fits_any = true,
        Some(p) if !p.is_empty() => q.fits.push(p.to_lowercase()),
        _ => {}
    }
    for (on, mark) in [
        (f.new_only, Mark::New),
        (f.untagged_only, Mark::Untagged),
        (f.problems_only, Mark::Problem),
        (f.favorites_only, Mark::Favorite),
    ] {
        if on && !q.marks.contains(&mark) {
            q.marks.push(mark);
        }
    }
    match f.printed {
        Some(PrintedFilter::Printed) => q.marks.push(Mark::Printed),
        Some(PrintedFilter::Unprinted) => q.marks.push(Mark::Unprinted),
        Some(PrintedFilter::Failed) => q.marks.push(Mark::Failed),
        None => {}
    }
    let mut sql = cx.lib.query_sql(&q, args);
    sql += &format!(" AND {}", cx.scope.sql("d.path", args));
    // What gets printed, unless other kinds were asked for.
    if q.kinds.is_empty() {
        sql += &format!(" AND f.kind IN ({INBOX_KINDS})");
    }
    let tag_in = |tags: &[String], args: &mut Vec<String>| -> String {
        let list: Vec<String> = tags.iter().map(|t| push(args, t.trim())).collect();
        format!(
            "SELECT ft.content_hash FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             WHERE t.name COLLATE NOCASE IN ({})",
            list.join(", ")
        )
    };
    if !f.tags_any.is_empty() {
        sql += &format!(" AND f.content_hash IN ({})", tag_in(&f.tags_any, args));
    }
    if !f.tags_none.is_empty() {
        sql += &format!(" AND (f.content_hash IS NULL OR f.content_hash NOT IN ({}))", tag_in(&f.tags_none, args));
    }
    if let Some(alias) = &f.library {
        let root =
            cx.scope.roots().iter().find(|r| r.alias == *alias).ok_or_else(|| ToolError::UnknownId(alias.clone()))?;
        sql += &format!(" AND {}", below_sql("d.path", &root.path, args));
    }
    if let Some(folder) = &f.folder {
        let path = cx.resolve(folder)?;
        sql += &format!(" AND {}", below_sql("d.path", &path, args));
    }
    if let Some(min) = f.min_size_mm {
        sql += &format!(" AND MAX(f.size_x, f.size_y, f.size_z) >= {}", min.max(0.0));
    }
    if let Some(max) = f.max_size_mm {
        sql += &format!(" AND MAX(f.size_x, f.size_y, f.size_z) <= {}", max.max(0.0));
    }
    if let Some(minutes) = f.max_print_minutes {
        // A model's print time is that of a G-code sliced from it, next to it.
        sql += &format!(
            " AND COALESCE(f.print_s, (SELECT MIN(g.print_s) FROM files g WHERE g.dir_id = f.dir_id \
               AND g.kind = 'gcode' AND g.print_s IS NOT NULL AND mg_sliced_from(g.name, f.name))) <= {}",
            minutes as u64 * 60
        );
    }
    if fits_any {
        sql += if cx.printers.is_empty() {
            " AND 0"
        } else {
            " AND f.size_x IS NOT NULL AND mg_fits_any(f.size_x, f.size_y, f.size_z)"
        };
    }
    Ok(sql)
}

/// Summaries of these files (those this scope may see and the index knows).
pub fn summaries(cx: &Context, paths: &[PathBuf]) -> ToolResult<Vec<ModelSummary>> {
    let facts = Facts::load(cx.lib)?;
    let mut out = Vec::new();
    for path in paths.iter().filter(|p| cx.scope.allows(p)) {
        if let Some(row) = cx.lib.ai_row_of(path)? {
            out.push(summary(cx, &row, &facts));
        }
    }
    Ok(out)
}

pub fn search_models(cx: &Context, f: &ModelFilter) -> ToolResult<Page<ModelSummary>> {
    let mut args = Vec::new();
    let conditions = filter_sql(cx, f, &mut args)?;
    let limit = f.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let offset: usize = f.cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
    let order = match f.sort {
        Some(SortBy::Newest) => "f.mtime_ns DESC, f.name",
        Some(SortBy::Size) => "f.size DESC, f.name",
        _ => "f.name COLLATE NOCASE, d.path",
    };
    let rows = cx.lib.ai_rows(&conditions, &args, order, limit + 1, offset)?;
    let next_cursor = (rows.len() > limit).then(|| (offset + limit).to_string());
    let facts = Facts::load(cx.lib)?;
    let items = rows.iter().take(limit).map(|r| summary(cx, r, &facts)).collect();
    Ok(Page { items, next_cursor })
}

fn row_for(cx: &Context, id: &str) -> ToolResult<(PathBuf, Row)> {
    let path = cx.resolve(id)?;
    let row = cx.lib.ai_row_of(&path)?.ok_or_else(|| ToolError::UnknownId(id.to_owned()))?;
    Ok((path, row))
}

/// The cached thumbnail of a row.
fn thumbnail(cx: &Context, row: &Row) -> Option<crate::cache::Thumbnail> {
    cx.thumbs?.load(row.meta_key.as_deref()?)?.ok()
}

/// PNG of `image`, at most `size` pixels on its longer side.
pub fn png(image: &RgbaImage, size: u32) -> Vec<u8> {
    let image = if image.width().max(image.height()) > size {
        let scale = size as f32 / image.width().max(image.height()) as f32;
        let (w, h) = ((image.width() as f32 * scale) as u32, (image.height() as f32 * scale) as u32);
        image::imageops::thumbnail(image, w.max(1), h.max(1))
    } else {
        image.clone()
    };
    let mut out = Vec::new();
    let _ = image.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

/// Everything about one file, and its picture (PNG) when asked for and allowed.
pub fn get_model(cx: &Context, id: &str, include_thumbnail: bool) -> ToolResult<(ModelDetail, Option<Vec<u8>>)> {
    let (path, row) = row_for(cx, id)?;
    let facts = Facts::load(cx.lib)?;
    let summary = summary(cx, &row, &facts);
    let thumb = thumbnail(cx, &row);
    let filaments = thumb
        .as_ref()
        .map(|t| t.meta.filaments.iter().map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}")).collect())
        .unwrap_or_default();
    let meta = (row.entry.kind == Kind::ThreeMf)
        .then(|| crate::archive::read(&path).ok())
        .flatten()
        .and_then(|bytes| crate::loaders::threemf::metadata(&bytes).ok())
        .unwrap_or_default();
    let note = cx.lib.notes_of(std::slice::from_ref(&path))?.0.map(|n| untrusted(&n, TEXT_CHARS));
    let lists = cx
        .lib
        .lists_of(std::slice::from_ref(&path))?
        .into_iter()
        .map(|(l, _)| if l.kind == ListKind::Favorites { "favorites".to_owned() } else { l.name })
        .collect();
    let copies = match &row.hash {
        Some(h) => cx
            .lib
            .paths_with(h)?
            .into_iter()
            .filter(|p| *p != path && cx.scope.allows(p))
            .map(|p| cx.id_of(&p))
            .collect(),
        None => Vec::new(),
    };
    let checks = row.hash.as_deref().map(|h| cx.lib.checks_by_hash(h)).transpose()?.flatten();
    let fit_per_printer = match row.size.filter(|_| row.entry.kind.is_model()) {
        Some(size) => printer::fits_all(size, &cx.printers).into_iter().map(|(p, f)| fit_out(&p.name, f)).collect(),
        None => Vec::new(),
    };
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let related_gcode = if row.entry.kind.is_model() { related_gcode(cx, &path, &dir)? } else { Vec::new() };
    let thumbnail_allowed = cx.scope.images_allowed(&path);
    let image = thumb.filter(|_| include_thumbnail && thumbnail_allowed).map(|t| png(&t.image, 384));
    let detail = ModelDetail {
        summary,
        path: path.display().to_string(),
        modified_unix: row
            .entry
            .modified
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
        content_hash: row.hash.clone(),
        title: meta.title,
        designer: meta.designer,
        application: meta.application,
        license: meta.license,
        description: meta.description.map(|d| untrusted(&d, TEXT_CHARS)),
        filaments,
        note,
        lists,
        copies,
        checks,
        fit_per_printer,
        related_gcode,
        folder_readme_untrusted: listing::readme_excerpt(&dir, TEXT_CHARS),
        thumbnail_allowed,
    };
    Ok((detail, image))
}

/// G-code next to a model whose name starts with the model's name ("rose.stl" → "rose_PLA_1h.gcode").
fn related_gcode(cx: &Context, model: &Path, dir: &Path) -> ToolResult<Vec<String>> {
    let stem = model.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut args = Vec::new();
    let dir_arg = push(&mut args, dir.to_string_lossy());
    let rows = cx.lib.ai_rows(&format!(" AND d.path = {dir_arg} AND f.kind = 'gcode'"), &args, "f.name", 50, 0)?;
    Ok(rows
        .into_iter()
        .filter(|r| !stem.is_empty() && listing::file_name(&r.entry.path).to_lowercase().starts_with(&stem))
        .map(|r| cx.id_of(&r.entry.path))
        .collect())
}

/// PNG pictures by id.
pub type Pictures = Vec<(String, Vec<u8>)>;

/// Pictures of files (PNG, at most `size` px), and the ids that have none or may not be shown.
pub fn get_thumbnails(cx: &Context, ids: &[String], size: u32) -> ToolResult<(Pictures, Vec<String>)> {
    if ids.len() > MAX_THUMBNAILS {
        return Err(ToolError::Invalid(format!("at most {MAX_THUMBNAILS} pictures per call")));
    }
    let size = size.clamp(64, 384);
    let (mut images, mut denied) = (Vec::new(), Vec::new());
    for id in ids {
        let picture =
            row_for(cx, id).ok().filter(|(p, _)| cx.scope.images_allowed(p)).and_then(|(_, r)| thumbnail(cx, &r));
        match picture {
            Some(t) => images.push((id.clone(), png(&t.image, size))),
            None => denied.push(id.clone()),
        }
    }
    Ok((images, denied))
}

pub fn folder_summary(cx: &Context, id: &str) -> ToolResult<FolderSummary> {
    let path = cx.resolve(id)?;
    let mut args = Vec::new();
    let mut within = format!(" AND {}", below_sql("d.path", &path, &mut args));
    within += &format!(" AND {}", cx.scope.sql("d.path", &mut args));
    let mut by_kind = Vec::new();
    {
        let sql = format!(
            "SELECT f.kind, COUNT(*) FROM files f JOIN dirs d ON d.id = f.dir_id WHERE 1 {within}
             GROUP BY f.kind ORDER BY 2 DESC"
        );
        let mut stmt = cx.lib.conn.prepare(&sql)?;
        for row in stmt.query_map(params_from_iter(&args), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (name, count) = row?;
            by_kind.push(TagCount { name, count: count as usize });
        }
    }
    let files = by_kind.iter().map(|k| k.count).sum();
    let common_tags = {
        let sql = format!(
            "SELECT t.name, COUNT(DISTINCT f.content_hash) FROM files f JOIN dirs d ON d.id = f.dir_id
             JOIN file_tags ft ON ft.content_hash = f.content_hash JOIN tags t ON t.id = ft.tag_id
             WHERE 1 {within} GROUP BY t.id ORDER BY 2 DESC, t.name LIMIT 10"
        );
        let mut stmt = cx.lib.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(&args), |r| {
            Ok(TagCount { name: r.get(0)?, count: r.get::<_, i64>(1)? as usize })
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    let new = cx.lib.ai_count(
        &format!("{within} AND f.content_hash IN (SELECT content_hash FROM inbox WHERE done_ns IS NULL)"),
        &args,
    )?;
    let subfolders =
        cx.lib.child_dirs(&path).into_iter().filter(|p| cx.scope.allows(p)).take(100).map(|p| cx.id_of(&p)).collect();
    let sample =
        search_models(cx, &ModelFilter { folder: Some(id.to_owned()), limit: Some(20), ..Default::default() })?.items;
    Ok(FolderSummary {
        id: cx.id_of(&path),
        files,
        by_kind,
        subfolders,
        common_tags,
        new,
        readme_untrusted: listing::readme_excerpt(&path, TEXT_CHARS),
        sample,
    })
}

/// Drops of new files waiting in the inbox (optionally only below `folder`).
pub fn list_inbox(cx: &Context, folder: Option<&str>, limit: usize) -> ToolResult<Vec<InboxGroupOut>> {
    let below = folder.map(|f| cx.resolve(f)).transpose()?;
    let roots: Vec<PathBuf> = cx.scope.roots().iter().map(|r| r.path.clone()).collect();
    let facts = Facts::load(cx.lib)?;
    let mut out = Vec::new();
    for g in cx.lib.inbox(&roots, limit.clamp(1, 2000))? {
        if !cx.scope.allows(&g.folder) || below.as_ref().is_some_and(|b| !g.folder.starts_with(b)) {
            continue;
        }
        let mut items = Vec::new();
        for item in &g.items {
            if cx.scope.allows(&item.entry.path)
                && let Some(row) = cx.lib.ai_row_of(&item.entry.path)?
            {
                items.push(summary(cx, &row, &facts));
            }
        }
        if !items.is_empty() {
            out.push(InboxGroupOut {
                folder: cx.id_of(&g.folder),
                newest_unix: g.newest_ns / 1_000_000_000,
                known_copies: g.known,
                items,
            });
        }
    }
    Ok(out)
}

/// Tags in use where this AI may look, with counts, by group.
fn tags_in_scope(cx: &Context) -> ToolResult<Vec<(String, usize)>> {
    let mut args = Vec::new();
    let scope = cx.scope.sql("d.path", &mut args);
    let sql = format!(
        "SELECT t.name, COUNT(DISTINCT ft.content_hash) FROM tags t JOIN file_tags ft ON ft.tag_id = t.id
         WHERE ft.content_hash IN (SELECT f.content_hash FROM files f JOIN dirs d ON d.id = f.dir_id WHERE {scope})
         GROUP BY t.id ORDER BY 2 DESC, t.name"
    );
    let mut stmt = cx.lib.conn.prepare(&sql)?;
    Ok(stmt
        .query_map(params_from_iter(&args), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?
        .collect::<Result<Vec<_>, _>>()?)
}

pub fn list_tags(cx: &Context) -> ToolResult<TagVocabulary> {
    let in_use = tags_in_scope(cx)?;
    let vocab = Vocabulary::new(cx.config, in_use.iter().map(|(t, _)| t.clone()).collect());
    let placing = cx.config.placement.enabled;
    let folder_of = |tag: &str| if placing { cx.config.folder_for_tag(tag) } else { None };
    let existing = in_use
        .iter()
        .map(|(name, count)| TagInUse {
            name: name.clone(),
            count: *count,
            group: vocab.group_of(name).to_owned(),
            folder: folder_of(name),
        })
        .collect();
    let suggested = vocab
        .suggested
        .iter()
        .map(|(t, group)| SuggestedOut {
            de: t.de.clone(),
            en: t.en.clone(),
            group: (*group).to_owned(),
            folder: folder_of(if vocab.german { &t.de } else { &t.en }),
        })
        .collect();
    Ok(TagVocabulary {
        existing,
        suggested,
        language: if vocab.german { "de" } else { "en" }.to_owned(),
        rules: "Prefer tags in use, then suggested ones in the user's language; a new tag only when nothing fits \
                (the review marks it as new). At most 8 tags per file, short names. Themes say where or what for, \
                properties how it prints, states where it stands."
            .to_owned(),
        placement: placing.then(|| {
            "Files in a library's import folder (library_overview: import_folder) are placed by tag: when you tag \
             one with a theme that has a folder, also propose_moves it into that folder (or a better existing \
             one). Files elsewhere stay where they are unless the user asks."
                .to_owned()
        }),
    })
}

/// Lists with files this AI may see (favourites always), with how many of them it sees.
pub fn list_lists(cx: &Context) -> ToolResult<Vec<ListSummary>> {
    let mut out = Vec::new();
    for l in cx.lib.lists()? {
        let visible = cx.lib.list_entries(l.id)?.iter().filter(|e| cx.scope.allows(&e.entry.path)).count();
        if visible == 0 && l.kind != ListKind::Favorites {
            continue;
        }
        out.push(ListSummary {
            name: if l.kind == ListKind::Favorites { "favorites".to_owned() } else { l.name },
            kind: l.kind.as_str().to_owned(),
            count: visible,
            finished: l.archived,
            saved_search: l.query,
        });
    }
    Ok(out)
}

pub fn get_list(cx: &Context, name: &str) -> ToolResult<ListDetail> {
    let wanted = name.trim();
    let favorites = wanted.eq_ignore_ascii_case("favorites");
    let found = cx
        .lib
        .lists()?
        .into_iter()
        .find(|l| l.name.eq_ignore_ascii_case(wanted) || (l.kind == ListKind::Favorites && favorites));
    let list = match found {
        Some(list) => list,
        // The favourites exist once the first file is starred; until then they are empty.
        None if favorites => {
            return Ok(ListDetail {
                name: "favorites".into(),
                kind: ListKind::Favorites.as_str().to_owned(),
                items: Vec::new(),
            });
        }
        None => return Err(ToolError::Invalid(format!("no list called {wanted:?} (list_lists shows them)"))),
    };
    let facts = Facts::load(cx.lib)?;
    let mut items = Vec::new();
    for e in cx.lib.list_entries(list.id)? {
        if cx.scope.allows(&e.entry.path)
            && let Some(row) = cx.lib.ai_row_of(&e.entry.path)?
        {
            items.push(ListItemOut { model: summary(cx, &row, &facts), quantity: e.quantity, done: e.done });
        }
    }
    if items.is_empty() && list.kind != ListKind::Favorites {
        // Nothing of it is visible: as if it did not exist.
        return Err(ToolError::Invalid(format!("no list called {wanted:?} (list_lists shows them)")));
    }
    Ok(ListDetail { name: list.name, kind: list.kind.as_str().to_owned(), items })
}

pub fn find_duplicates(cx: &Context, library: Option<&str>, limit: usize) -> ToolResult<Vec<DuplicateOut>> {
    let root = match library {
        Some(alias) => Some(
            cx.scope
                .roots()
                .iter()
                .find(|r| r.alias == alias)
                .ok_or_else(|| ToolError::UnknownId(alias.to_owned()))?
                .path
                .clone(),
        ),
        None => None,
    };
    let mut out = Vec::new();
    for g in cx.lib.duplicate_groups()? {
        let files: Vec<&Entry> = g
            .files
            .iter()
            .filter(|f| cx.scope.allows(&f.path) && root.as_ref().is_none_or(|r| f.path.starts_with(r)))
            .collect();
        if files.len() < 2 {
            continue;
        }
        let total: u64 = files.iter().map(|f| f.size).sum();
        let largest = files.iter().map(|f| f.size).max().unwrap_or(0);
        out.push(DuplicateOut {
            exact: g.exact,
            wasted_bytes: total - largest,
            files: files.iter().map(|f| cx.id_of(&f.path)).collect(),
        });
        if out.len() >= limit.clamp(1, 500) {
            break;
        }
    }
    Ok(out)
}

/// Folders whose name contains `needle`, for references in the chat (within the scope).
pub fn find_folders(cx: &Context, needle: &str, limit: usize) -> ToolResult<Vec<std::path::PathBuf>> {
    let mut args = vec![format!("%{}%", needle.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))];
    let scope = cx.scope.sql("d.path", &mut args);
    let sql = format!(
        "SELECT d.path FROM dirs d WHERE d.path LIKE ?1 ESCAPE '\\' AND {scope} ORDER BY length(d.path) LIMIT {}",
        limit * 4
    );
    let mut stmt = cx.lib.conn.prepare(&sql)?;
    let paths: Vec<std::path::PathBuf> = stmt
        .query_map(params_from_iter(&args), |r| r.get::<_, String>(0))?
        .filter_map(Result::ok)
        .map(Into::into)
        .collect();
    let needle = needle.to_lowercase();
    // Only where the name itself matches, not a folder above it.
    Ok(paths
        .into_iter()
        .filter(|p| listing::file_name(p).to_lowercase().contains(&needle) && cx.scope.allows(p))
        .take(limit)
        .collect())
}

pub fn list_printers(cx: &Context) -> Vec<PrinterOut> {
    cx.printers
        .iter()
        .enumerate()
        .map(|(i, p)| PrinterOut {
            name: p.name.clone(),
            bed_mm: [p.x, p.y, p.z],
            nozzle_mm: p.nozzle,
            materials: p.materials.clone(),
            main: i == 0,
        })
        .collect()
}

pub fn check_printability(cx: &Context, ids: &[String]) -> ToolResult<Vec<CheckOut>> {
    if ids.len() > MAX_CHECKS {
        return Err(ToolError::Invalid(format!("at most {MAX_CHECKS} files per call")));
    }
    let mut out = Vec::new();
    for id in ids {
        let Ok((_, row)) = row_for(cx, id) else {
            out.push(CheckOut {
                id: id.clone(),
                checks: None,
                problems: vec!["unknown_id".into()],
                fit_per_printer: Vec::new(),
            });
            continue;
        };
        let checks = row.hash.as_deref().map(|h| cx.lib.checks_by_hash(h)).transpose()?.flatten();
        let problems = checks::problems(row.entry.kind, row.size, checks.as_ref(), &cx.printers, cx.config.model_units)
            .iter()
            .map(|p| p.code().to_owned())
            .collect();
        let fit_per_printer = match row.size.filter(|_| row.entry.kind.is_model()) {
            Some(size) => printer::fits_all(size, &cx.printers).into_iter().map(|(p, f)| fit_out(&p.name, f)).collect(),
            None => Vec::new(),
        };
        out.push(CheckOut { id: id.clone(), checks, problems, fit_per_printer });
    }
    Ok(out)
}

pub fn library_overview(cx: &Context) -> ToolResult<Overview> {
    let mut libraries = Vec::new();
    for root in cx.scope.roots() {
        let mut args = Vec::new();
        let mut within = format!(" AND {}", below_sql("d.path", &root.path, &mut args));
        within += &format!(" AND {}", cx.scope.sql("d.path", &mut args));
        let files = cx.lib.ai_count(&within, &args)?;
        let models = cx.lib.ai_count(&format!("{within} AND f.kind IN ({INBOX_KINDS})"), &args)?;
        let landing = root.path.join(cx.config.landing_name());
        let import_folder = (cx.config.placement.enabled && landing.is_dir()).then(|| cx.id_of(&landing));
        libraries.push(LibraryOut {
            alias: root.alias.clone(),
            files,
            models,
            thumbnails_allowed: root.images,
            import_folder,
        });
    }
    let count = |f: ModelFilter| -> ToolResult<usize> {
        let mut args = Vec::new();
        let conditions = filter_sql(cx, &f, &mut args)?;
        Ok(cx.lib.ai_count(&conditions, &args)?)
    };
    let index_age_s = cx.lib.meta_get("last_scan_ns").and_then(|v| v.parse::<u64>().ok()).and_then(|ns| {
        cx.now.duration_since(SystemTime::UNIX_EPOCH + Duration::from_nanos(ns)).ok().map(|d| d.as_secs())
    });
    Ok(Overview {
        libraries,
        inbox_waiting: count(ModelFilter { new_only: true, ..Default::default() })?,
        untagged_models: count(ModelFilter { untagged_only: true, ..Default::default() })?,
        with_problems: count(ModelFilter { problems_only: true, ..Default::default() })?,
        printers: cx.printers.iter().map(|p| p.name.clone()).collect(),
        top_tags: tags_in_scope(cx)?.into_iter().take(15).map(|(name, count)| TagCount { name, count }).collect(),
        lists: list_lists(cx)?.len(),
        index_age_s,
        language: cx.config.language.clone().unwrap_or_else(|| "system".to_owned()),
    })
}
