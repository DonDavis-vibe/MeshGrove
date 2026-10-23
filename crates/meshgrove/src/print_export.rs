//! Print lists as files: a CSV for spreadsheets and a PDF checklist with pictures, sizes,
//! filament colours and a box to tick for every copy.

use std::path::PathBuf;

use image::RgbaImage;
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, TextDirection};

/// One entry of a print list.
pub struct Row {
    pub name: String,
    pub folder: String,
    pub quantity: u32,
    pub done: u32,
    pub size_mm: Option<[f32; 3]>,
    pub filaments: Vec<[u8; 3]>,
    /// The tile picture (PDF only).
    pub picture: Option<RgbaImage>,
}

/// Column titles of the CSV, in the UI language.
pub struct CsvHeader<'a> {
    pub name: &'a str,
    pub folder: &'a str,
    pub quantity: &'a str,
    pub done: &'a str,
    pub size: &'a str,
    pub filaments: &'a str,
}

/// The list as CSV. `separator` is `;` where spreadsheets expect it (German) and `,` elsewhere.
/// "124.8 × 144.1 × 50.0" (millimetres) or "4.91 × 5.67 × 1.97" (inches), with the unit if asked.
fn size_text(size_mm: [f32; 3], with_unit: bool) -> String {
    let units = meshgrove_core::config::display_units();
    let decimals = if units == meshgrove_core::config::DisplayUnits::Metric { 1 } else { 2 };
    let [x, y, z] = size_mm.map(|v| format!("{:.decimals$}", units.from_mm(v)));
    if with_unit { format!("{x} × {y} × {z} {}", units.label()) } else { format!("{x} × {y} × {z}") }
}

pub fn csv(rows: &[Row], header: &CsvHeader, separator: char) -> String {
    let field = |s: &str| {
        if s.contains([separator, '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
    };
    let line = |fields: &[String]| fields.iter().map(|f| field(f)).collect::<Vec<_>>().join(&separator.to_string());
    let mut out = line(
        &[header.name, header.folder, header.quantity, header.done, header.size, header.filaments].map(str::to_owned),
    );
    out.push_str("\r\n");
    for r in rows {
        let size = r.size_mm.map(|s| size_text(s, false)).unwrap_or_default();
        let filaments: Vec<String> =
            r.filaments.iter().map(|[red, g, b]| format!("#{red:02x}{g:02x}{b:02x}")).collect();
        out.push_str(&line(&[
            r.name.clone(),
            r.folder.clone(),
            r.quantity.to_string(),
            r.done.to_string(),
            size,
            filaments.join(" "),
        ]));
        out.push_str("\r\n");
    }
    out
}

/// The text of the PDF in the UI language.
pub struct PdfText<'a> {
    pub title: &'a str,
    /// Date and totals under the title.
    pub subtitle: &'a str,
    /// "Page" for the footer.
    pub page: &'a str,
}

const PAGE: (f32, f32) = (595.0, 842.0); // A4 in points
const MARGIN: f32 = 42.0;
const ROW: f32 = 74.0;
const PICTURE: f32 = 60.0;
const BOX: f32 = 11.0;
/// Boxes drawn for a row at most; more copies show as "× n" next to them.
const MAX_BOXES: u32 = 10;

/// The list as a PDF checklist on A4. `font` is a TrueType/OpenType font for all text.
pub fn pdf(rows: &[Row], text: &PdfText, font: Vec<u8>) -> Result<Vec<u8>, String> {
    let font = Font::new(font.into(), 0).ok_or("the font could not be read")?;
    let mut document = Document::new();
    let per_page = (((PAGE.1 - 2.0 * MARGIN - 70.0) / ROW).floor() as usize).max(1);
    let pages = rows.len().div_ceil(per_page).max(1);
    for page_no in 0..pages {
        let mut page = document.start_page_with(PageSettings::from_wh(PAGE.0, PAGE.1).ok_or("bad page size")?);
        let mut s = page.surface();
        let mut y = MARGIN;
        if page_no == 0 {
            write(&mut s, &font, 20.0, Point::from_xy(MARGIN, y + 20.0), text.title, INK);
            write(&mut s, &font, 10.0, Point::from_xy(MARGIN, y + 38.0), text.subtitle, GREY);
            y += 60.0;
        } else {
            write(&mut s, &font, 10.0, Point::from_xy(MARGIN, y + 10.0), text.title, GREY);
            y += 24.0;
        }
        for row in rows.iter().skip(page_no * per_page).take(per_page) {
            draw_row(&mut s, &font, row, y);
            y += ROW;
        }
        let footer = format!("Meshgrove · {} {} / {}", text.page, page_no + 1, pages);
        write(&mut s, &font, 8.0, Point::from_xy(MARGIN, PAGE.1 - MARGIN / 2.0), &footer, GREY);
        s.finish();
        page.finish();
    }
    document.finish().map_err(|e| format!("{e:?}"))
}

const INK: (u8, u8, u8) = (30, 30, 30);
const GREY: (u8, u8, u8) = (110, 110, 110);
const LINE: (u8, u8, u8) = (210, 210, 210);

fn draw_row(s: &mut Surface, font: &Font, row: &Row, top: f32) {
    let left = MARGIN;
    // Thin line between rows.
    fill_rect(s, Rect::from_xywh(left, top + ROW - 1.0, PAGE.0 - 2.0 * MARGIN, 0.6), LINE);
    if let Some(picture) = &row.picture {
        let (w, h) = (picture.width(), picture.height());
        let image = Image::from_rgba8(picture.as_raw().clone(), w, h);
        let scale = PICTURE / w.max(h) as f32;
        let (dw, dh) = (w as f32 * scale, h as f32 * scale);
        let at = Transform::from_translate(left + (PICTURE - dw) / 2.0, top + 6.0 + (PICTURE - dh) / 2.0);
        s.push_transform(&at);
        if let Some(size) = Size::from_wh(dw, dh) {
            s.draw_image(image, size);
        }
        s.pop();
    }
    let text_left = left + PICTURE + 14.0;
    let boxes = row.quantity.min(MAX_BOXES);
    let boxes_width = boxes as f32 * (BOX + 4.0) + if row.quantity > MAX_BOXES { 34.0 } else { 0.0 };
    let text_width = PAGE.0 - MARGIN - boxes_width - 16.0 - text_left;
    write(s, font, 12.0, Point::from_xy(text_left, top + 22.0), &fit(&row.name, 12.0, text_width), INK);
    write(s, font, 8.5, Point::from_xy(text_left, top + 36.0), &fit(&row.folder, 8.5, text_width), GREY);
    let mut x = text_left;
    if let Some([sx, sy, sz]) = row.size_mm {
        let size = size_text([sx, sy, sz], true);
        write(s, font, 9.0, Point::from_xy(x, top + 52.0), &size, INK);
        x += size.chars().count() as f32 * 4.6 + 10.0;
    }
    for &[r, g, b] in row.filaments.iter().take(12) {
        fill_rect(s, Rect::from_xywh(x, top + 44.0, 9.0, 9.0), (r, g, b));
        stroke_rect(s, Rect::from_xywh(x, top + 44.0, 9.0, 9.0), GREY, 0.5);
        x += 12.0;
    }
    // One box per copy, the printed ones ticked.
    let mut bx = PAGE.0 - MARGIN - boxes_width;
    let by = top + ROW / 2.0 - BOX / 2.0;
    for i in 0..boxes {
        let square = Rect::from_xywh(bx, by, BOX, BOX);
        stroke_rect(s, square, INK, 0.8);
        if i < row.done {
            tick(s, bx, by);
        }
        bx += BOX + 4.0;
    }
    if row.quantity > MAX_BOXES {
        write(s, font, 9.0, Point::from_xy(bx + 2.0, by + 9.0), &format!("× {}", row.quantity), INK);
    }
}

/// Cuts `text` with "…" so it fits `width` (estimated from the font size; the font is not measured).
fn fit(text: &str, size: f32, width: f32) -> String {
    let max = (width / (size * 0.52)).floor().max(4.0) as usize;
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

fn write(s: &mut Surface, font: &Font, size: f32, at: Point, text: &str, color: (u8, u8, u8)) {
    s.set_fill(Some(fill(color)));
    s.draw_text(at, font.clone(), size, text, false, TextDirection::Auto);
}

fn fill(color: (u8, u8, u8)) -> Fill {
    Fill {
        paint: rgb::Color::new(color.0, color.1, color.2).into(),
        opacity: NormalizedF32::ONE,
        rule: Default::default(),
    }
}

fn fill_rect(s: &mut Surface, rect: Option<Rect>, color: (u8, u8, u8)) {
    let Some(rect) = rect else { return };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    if let Some(path) = pb.finish() {
        s.set_stroke(None);
        s.set_fill(Some(fill(color)));
        s.draw_path(&path);
    }
}

fn stroke_rect(s: &mut Surface, rect: Option<Rect>, color: (u8, u8, u8), width: f32) {
    let Some(rect) = rect else { return };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    if let Some(path) = pb.finish() {
        s.set_fill(None);
        s.set_stroke(Some(Stroke {
            paint: rgb::Color::new(color.0, color.1, color.2).into(),
            width,
            ..Default::default()
        }));
        s.draw_path(&path);
        s.set_stroke(None);
    }
}

/// A tick in the box whose top-left corner is (x, y).
fn tick(s: &mut Surface, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + 2.2, y + 5.8);
    pb.line_to(x + 4.6, y + 8.4);
    pb.line_to(x + 9.0, y + 2.4);
    if let Some(path) = pb.finish() {
        s.set_fill(None);
        s.set_stroke(Some(Stroke { paint: rgb::Color::new(30, 120, 50).into(), width: 1.6, ..Default::default() }));
        s.draw_path(&path);
        s.set_stroke(None);
    }
}

/// The desktop's UI font file when there is one (Noto Sans on KDE, Segoe UI on Windows), else
/// the font egui brings along.
pub fn font(system: Option<PathBuf>) -> Vec<u8> {
    system.and_then(|p| std::fs::read(p).ok()).unwrap_or_else(|| epaint_default_fonts::UBUNTU_LIGHT.to_vec())
}

/// `name` made safe as part of a file name.
pub fn file_name_part(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            Row {
                name: "Body2.stl".into(),
                folder: "/lib/Airsoft Gear".into(),
                quantity: 3,
                done: 1,
                size_mm: Some([124.8, 144.1, 50.0]),
                filaments: vec![[255, 255, 255], [200, 20, 40]],
                picture: Some(RgbaImage::from_pixel(8, 4, image::Rgba([90, 90, 90, 255]))),
            },
            Row {
                name: "Lid; \"round\".stl".into(),
                folder: "/lib".into(),
                quantity: 14,
                done: 0,
                size_mm: None,
                filaments: Vec::new(),
                picture: None,
            },
        ]
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        let header = CsvHeader {
            name: "Name",
            folder: "Ordner",
            quantity: "Stück",
            done: "Fertig",
            size: "Maße (mm)",
            filaments: "Filamente",
        };
        let out = csv(&rows(), &header, ';');
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Name;Ordner;Stück;Fertig;Maße (mm);Filamente");
        assert_eq!(lines[1], "Body2.stl;/lib/Airsoft Gear;3;1;124.8 × 144.1 × 50.0;#ffffff #c81428");
        assert_eq!(lines[2], "\"Lid; \"\"round\"\".stl\";/lib;14;0;;");
        let commas = csv(&rows(), &header, ',');
        assert_eq!(
            commas.lines().nth(1).unwrap(),
            "Body2.stl,/lib/Airsoft Gear,3,1,124.8 × 144.1 × 50.0,#ffffff #c81428"
        );
    }

    #[test]
    fn pdf_is_written_with_the_bundled_font() {
        let text = PdfText { title: "Weihnachtsmarkt", subtitle: "23.10.2026 · 2 Modelle, 17 Stück", page: "Seite" };
        let many: Vec<Row> = (0..25).flat_map(|_| rows()).collect();
        let one = pdf(&rows(), &text, font(None)).unwrap();
        let bytes = pdf(&many, &text, font(None)).unwrap();
        assert!(one.starts_with(b"%PDF") && bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > one.len(), "50 rows take more pages than 2");
    }

    #[test]
    fn long_names_are_cut_and_file_names_cleaned() {
        assert_eq!(fit("short.stl", 12.0, 300.0), "short.stl");
        let cut = fit(&"x".repeat(200), 12.0, 100.0);
        assert!(cut.ends_with('…') && cut.chars().count() < 30);
        assert_eq!(file_name_part("Markt: 3/4"), "Markt- 3-4");
    }
}
