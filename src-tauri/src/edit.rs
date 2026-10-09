//! Content editing: text blocks, images and fonts.
//!
//! Many PDFs (Chromium/Skia output in particular) store every glyph as its own
//! text object, so the editing unit is a *block*: glyph objects grouped into
//! lines and lines into paragraphs. Editing a block removes its glyph objects
//! and lays the new text out again with fresh text objects.
//!
//! Fonts: an embedded subset font only contains the glyphs the document
//! already used, so each character keeps its original font when that font has
//! the glyph, and falls back to a matching standard font (or a system TrueType
//! font outside WinAnsi) when it does not.
//!
//! pdfium-render 0.8.37 destroys a removed `PdfPageObject` twice on drop, which
//! crashes PDFium, so removed objects are leaked with `mem::forget` (a few
//! hundred bytes each).
//!
//! Layout happens in PDF user space (y up), so it is unaffected by /Rotate.

use crate::engine::{err, Geom};
use pdfium_render::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// A run of text drawn with one style, as found on the page.
#[derive(Clone, PartialEq)]
pub struct Style {
    pub font_name: String,
    /// Effective size in points (font size × text matrix scale).
    pub size: f32,
    pub color: (u8, u8, u8, u8),
    /// Index of an object drawn with this style, used to fetch its font.
    rep: usize,
    pub embedded: bool,
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
}

struct Glyph {
    index: usize,
    text: String,
    style: usize,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    /// Text origin (start of baseline).
    x: f32,
    y: f32,
}

struct Line {
    glyphs: Vec<usize>, // into the glyph vec, left to right
    y: f32,
    size: f32,
    right: f32,
    x: f32,
}

/// A detected, editable text block (internal form).
pub struct BlockDef {
    pub objects: Vec<usize>,
    /// Characters with the style index each was drawn with.
    pub chars: Vec<(char, usize)>,
    pub x: f32,
    pub baseline: f32,
    pub width: f32,
    pub line_gap: f32,
    pub lines: usize,
    /// [start, end) of each original line within `chars`, and its drawn width.
    pub line_ranges: Vec<(usize, usize)>,
    pub line_widths: Vec<f32>,
    /// left, bottom, right, top in user space
    pub bbox: [f32; 4],
}

pub struct ImageDef {
    pub index: usize,
    pub bbox: [f32; 4],
    pub px: (u32, u32),
}

/// Everything editable on one page.
pub struct PageModel {
    pub styles: Vec<Style>,
    pub blocks: Vec<BlockDef>,
    pub images: Vec<ImageDef>,
}

// ---- What the UI sees ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBlockInfo {
    pub id: usize,
    /// left, top, right, bottom in displayed page space
    pub rect: [f32; 4],
    pub text: String,
    pub size: f32,
    pub line_height: f32,
    pub family: &'static str,
    pub bold: bool,
    pub italic: bool,
    pub color: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub id: usize,
    pub rect: [f32; 4],
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageLayout {
    pub blocks: Vec<TextBlockInfo>,
    pub images: Vec<ImageInfo>,
}

impl Style {
    fn family(&self) -> &'static str {
        if self.mono {
            "monospace"
        } else if self.serif {
            "serif"
        } else {
            "sans-serif"
        }
    }
}

fn rect_of(obj: &PdfPageObject) -> Option<[f32; 4]> {
    let r = obj.bounds().ok()?.to_rect();
    Some([r.left().value, r.bottom().value, r.right().value, r.top().value])
}

fn font_traits(name: &str, font: &PdfFont) -> (bool, bool, bool, bool) {
    let n = name.to_lowercase();
    let n = n.split('+').next_back().unwrap_or(&n);
    let bold = ["bold", "semibold", "black", "heavy", "demi"].iter().any(|k| n.contains(k))
        || font.weight().map(|w| matches!(w, PdfFontWeight::Weight600 | PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900)).unwrap_or(false)
        || font.is_bold_reenforced();
    let italic = n.contains("italic") || n.contains("oblique") || font.is_italic();
    let mono = font.is_fixed_pitch()
        || ["mono", "courier", "consol", "code"].iter().any(|k| n.contains(k));
    let serif = !mono
        && (n.contains("serif") && !n.contains("sans")
            || ["times", "georgia", "garamond", "cambria", "book", "minion", "roman"].iter().any(|k| n.contains(k))
            || (font.is_serif() && !n.contains("sans")));
    (bold, italic, serif, mono)
}

/// Reads the page's text glyphs and images into an editable model.
pub fn analyse(page: &PdfPage) -> PageModel {
    let mut styles: Vec<Style> = Vec::new();
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut empties: Vec<(usize, f32, f32)> = Vec::new(); // zero-width objects: index, x, y
    let mut images = Vec::new();
    // One text page for all objects: PdfPageTextObject::text() would load a
    // new one per call, which is quadratic on pages with a glyph per object.
    let text_page = page.text().ok();

    for (index, obj) in page.objects().iter().enumerate() {
        match &obj {
            PdfPageObject::Text(t) => {
                if t.render_mode() == PdfPageTextRenderMode::Invisible {
                    continue;
                }
                let Ok(m) = t.matrix() else { continue };
                // Only upright, unmirrored text is editable for now.
                if m.b().abs() > 1e-3 || m.c().abs() > 1e-3 || m.a() <= 0.0 || m.d() <= 0.0 {
                    continue;
                }
                let text = match &text_page {
                    Some(tp) => tp.for_object(t),
                    None => t.text(),
                };
                if text.is_empty() {
                    empties.push((index, m.e(), m.f()));
                    continue;
                }
                let Some(b) = rect_of(&obj) else { continue };
                let font = t.font();
                let name = font.name();
                let c = obj.fill_color().unwrap_or(PdfColor::new(0, 0, 0, 255));
                let size = t.unscaled_font_size().value * m.d();
                if size <= 0.1 {
                    continue;
                }
                let (bold, italic, serif, mono) = font_traits(&name, &font);
                let style = Style {
                    font_name: name,
                    size,
                    color: (c.red(), c.green(), c.blue(), c.alpha()),
                    rep: index,
                    embedded: font.is_embedded().unwrap_or(false),
                    bold,
                    italic,
                    serif,
                    mono,
                };
                let si = match styles.iter().position(|s| {
                    s.font_name == style.font_name
                        && (s.size - style.size).abs() < 0.05
                        && s.color == style.color
                }) {
                    Some(i) => i,
                    None => {
                        styles.push(style);
                        styles.len() - 1
                    }
                };
                glyphs.push(Glyph {
                    index,
                    text,
                    style: si,
                    left: b[0],
                    bottom: b[1],
                    right: b[2],
                    top: b[3],
                    x: m.e(),
                    y: m.f(),
                });
            }
            PdfPageObject::Image(im) => {
                if let Some(b) = rect_of(&obj) {
                    images.push(ImageDef {
                        index,
                        bbox: b,
                        px: (im.width().unwrap_or(0) as u32, im.height().unwrap_or(0) as u32),
                    });
                }
            }
            _ => {}
        }
    }

    let lines = build_lines(&glyphs, &styles);
    let mut blocks = build_blocks(&lines, &glyphs, &styles);

    // Zero-width objects (spacing artifacts) go with the block whose line they sit on.
    for (index, x, y) in empties {
        if let Some(b) = blocks.iter_mut().find(|b| {
            x >= b.bbox[0] - 1.0 && x <= b.bbox[2] + 1.0 && y >= b.bbox[1] - 1.0 && y <= b.bbox[3] + 1.0
        }) {
            b.objects.push(index);
        }
    }
    PageModel { styles, blocks, images }
}

fn build_lines(glyphs: &[Glyph], styles: &[Style]) -> Vec<Line> {
    let mut order: Vec<usize> = (0..glyphs.len()).collect();
    order.sort_by(|&a, &b| glyphs[b].y.total_cmp(&glyphs[a].y));

    // Rows: glyphs sharing a baseline (within a fraction of the font size).
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for g in order {
        let size = styles[glyphs[g].style].size;
        match rows.last_mut() {
            Some(row) if (glyphs[row[0]].y - glyphs[g].y).abs() < size * 0.35 => row.push(g),
            _ => rows.push(vec![g]),
        }
    }

    // Split rows into lines at large horizontal gaps or size changes.
    let mut lines = Vec::new();
    for mut row in rows {
        row.sort_by(|&a, &b| glyphs[a].x.total_cmp(&glyphs[b].x));
        let mut cur: Option<Line> = None;
        for g in row {
            let gl = &glyphs[g];
            let size = styles[gl.style].size;
            if let Some(line) = &mut cur {
                let gap = gl.left - line.right;
                let same_size = (size - line.size).abs() / line.size < 0.3;
                if same_size && gap < line.size * 1.2 && gap > -line.size * 0.5 {
                    line.glyphs.push(g);
                    line.right = line.right.max(gl.right);
                    line.size = line.size.max(size);
                    continue;
                }
                lines.push(cur.take().unwrap());
            }
            cur = Some(Line { glyphs: vec![g], y: gl.y, size, right: gl.right, x: gl.x });
        }
        lines.extend(cur);
    }
    lines
}

fn dominant_style(line: &Line, glyphs: &[Glyph]) -> usize {
    let mut count: HashMap<usize, usize> = HashMap::new();
    for &g in &line.glyphs {
        *count.entry(glyphs[g].style).or_default() += glyphs[g].text.chars().count();
    }
    count.into_iter().max_by_key(|&(_, n)| n).map(|(s, _)| s).unwrap_or(0)
}

fn build_blocks(lines: &[Line], glyphs: &[Glyph], styles: &[Style]) -> Vec<BlockDef> {
    // Group lines (already top to bottom) into paragraphs.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let size = styles[dominant_style(line, glyphs)].size;
        let target = groups.iter_mut().rev().find(|grp| {
            let last = &lines[*grp.last().unwrap()];
            let last_size = styles[dominant_style(last, glyphs)].size;
            let gap = last.y - line.y;
            let aligned = (last.x - line.x).abs() < 2.5;
            let similar = (last_size - size).abs() / size < 0.08;
            let spacing_ok = gap > size * 0.9 && gap < size * 1.9;
            let consistent = grp.len() < 2 || {
                let prev = &lines[grp[grp.len() - 2]];
                ((prev.y - last.y) - gap).abs() < size * 0.2
            };
            aligned && similar && spacing_ok && consistent
        });
        match target {
            Some(grp) => grp.push(li),
            None => groups.push(vec![li]),
        }
    }

    groups
        .into_iter()
        .map(|grp| {
            let mut objects = Vec::new();
            let mut chars: Vec<(char, usize)> = Vec::new();
            let mut line_ranges = Vec::new();
            let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for (n, &li) in grp.iter().enumerate() {
                let line = &lines[li];
                if n > 0 {
                    // Soft line break: join with a space unless one is there or it ends with a hyphen.
                    if let Some(&(c, s)) = chars.last() {
                        if !c.is_whitespace() && c != '-' && c != '\u{ad}' {
                            chars.push((' ', s));
                        }
                    }
                }
                let line_start = chars.len();
                let mut prev_right: Option<f32> = None;
                for &g in &line.glyphs {
                    let gl = &glyphs[g];
                    let size = styles[gl.style].size;
                    // Words separated only by positioning get an explicit space.
                    if let (Some(pr), Some(&(c, s))) = (prev_right, chars.last()) {
                        let starts_space = gl.text.starts_with(char::is_whitespace);
                        if gl.left - pr > size * 0.22 && !c.is_whitespace() && !starts_space {
                            chars.push((' ', s));
                        }
                    }
                    chars.extend(gl.text.chars().map(|c| (c, gl.style)));
                    objects.push(gl.index);
                    bbox[0] = bbox[0].min(gl.left);
                    bbox[1] = bbox[1].min(gl.bottom);
                    bbox[2] = bbox[2].max(gl.right);
                    bbox[3] = bbox[3].max(gl.top);
                    prev_right = Some(gl.right);
                }
                line_ranges.push((line_start, chars.len()));
            }
            let first = &lines[grp[0]];
            let last = &lines[*grp.last().unwrap()];
            let line_gap = if grp.len() > 1 {
                (first.y - last.y) / (grp.len() - 1) as f32
            } else {
                first.size * 1.25
            };
            let x = grp.iter().map(|&l| lines[l].x).fold(f32::MAX, f32::min);
            let width = grp.iter().map(|&l| lines[l].right).fold(f32::MIN, f32::max) - x;
            // Trim trailing whitespace from the editable text.
            while chars.last().is_some_and(|(c, _)| c.is_whitespace()) {
                chars.pop();
            }
            let line_widths = grp.iter().map(|&l| lines[l].right - lines[l].x).collect();
            let len = chars.len();
            for r in &mut line_ranges {
                *r = (r.0.min(len), r.1.min(len));
            }
            BlockDef { objects, chars, x, baseline: first.y, width, line_gap, lines: grp.len(), line_ranges, line_widths, bbox }
        })
        .filter(|b| !b.chars.is_empty())
        .collect()
}

fn css_color(c: (u8, u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

impl PageModel {
    pub fn layout(&self, geom: &Geom) -> PageLayout {
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(id, b)| {
                let mut count: HashMap<usize, usize> = HashMap::new();
                for &(_, s) in &b.chars {
                    *count.entry(s).or_default() += 1;
                }
                let s = &self.styles[count.into_iter().max_by_key(|&(_, n)| n).unwrap().0];
                TextBlockInfo {
                    id,
                    rect: geom.user_rect(b.bbox),
                    text: b.chars.iter().map(|&(c, _)| c).collect(),
                    size: s.size,
                    line_height: b.line_gap,
                    family: s.family(),
                    bold: s.bold,
                    italic: s.italic,
                    color: css_color(s.color),
                }
            })
            .collect();
        let images = self
            .images
            .iter()
            .enumerate()
            .map(|(id, im)| ImageInfo { id, rect: geom.user_rect(im.bbox), width: im.px.0, height: im.px.1 })
            .collect();
        PageLayout { blocks, images }
    }
}

// ---- Fonts ----

/// Standard fallback fonts, chosen to match the original style.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fallback {
    Builtin(u8), // index into BUILTINS: family*4 + bold*1 + italic*2
    System(u8),  // same indexing, system TrueType for non-WinAnsi text
}

const BUILTINS: [PdfFontBuiltin; 12] = [
    PdfFontBuiltin::Helvetica,
    PdfFontBuiltin::HelveticaBold,
    PdfFontBuiltin::HelveticaOblique,
    PdfFontBuiltin::HelveticaBoldOblique,
    PdfFontBuiltin::TimesRoman,
    PdfFontBuiltin::TimesBold,
    PdfFontBuiltin::TimesItalic,
    PdfFontBuiltin::TimesBoldItalic,
    PdfFontBuiltin::Courier,
    PdfFontBuiltin::CourierBold,
    PdfFontBuiltin::CourierOblique,
    PdfFontBuiltin::CourierBoldOblique,
];

#[cfg(windows)]
const SYSTEM_FONTS: [&str; 12] = [
    "arial.ttf", "arialbd.ttf", "ariali.ttf", "arialbi.ttf",
    "times.ttf", "timesbd.ttf", "timesi.ttf", "timesbi.ttf",
    "cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf",
];

fn system_font_path(i: u8) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or("C:\\Windows".into());
        let p = dir.join("Fonts").join(SYSTEM_FONTS[i as usize]);
        // Fall back to the regular face of the family, then to Arial.
        [p, dir.join("Fonts").join(SYSTEM_FONTS[(i & !3) as usize]), dir.join("Fonts").join("arial.ttf")]
            .into_iter()
            .find(|p| p.exists())
    }
    #[cfg(not(windows))]
    {
        let _ = i;
        None
    }
}

fn style_slot(family: &str, bold: bool, italic: bool) -> u8 {
    let f = match family {
        "serif" => 1,
        "monospace" => 2,
        _ => 0,
    };
    f * 4 + bold as u8 + 2 * italic as u8
}

/// Characters that the standard 14 fonts can show (WinAnsiEncoding).
pub fn is_winansi(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7e | 0xa0..=0xff)
        || "€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".contains(c)
}

/// Per-document font state shared between edits.
#[derive(Default)]
pub struct Fonts {
    /// Characters known to exist in each embedded font (by font name).
    pub coverage: Option<HashMap<String, HashSet<char>>>,
    pub loaded: HashMap<Fallback, PdfFontToken>,
}

/// Collects, per font, the characters the document already draws with it.
/// For an embedded subset font this is exactly the set of glyphs it contains.
pub fn scan_coverage(doc: &PdfDocument) -> HashMap<String, HashSet<char>> {
    let mut map: HashMap<String, HashSet<char>> = HashMap::new();
    for page in doc.pages().iter() {
        let Ok(text) = page.text() else { continue };
        for ch in text.chars().iter() {
            if ch.is_generated().unwrap_or(true) {
                continue;
            }
            if let Some(c) = ch.unicode_char() {
                map.entry(ch.font_name()).or_default().insert(c);
            }
        }
    }
    map
}

/// Which font draws `c` in style `s`: the original, or a fallback.
fn resolve(s: &Style, c: char, coverage: &HashMap<String, HashSet<char>>) -> Option<Fallback> {
    let covered = if s.embedded {
        coverage.get(&s.font_name).is_some_and(|set| set.contains(&c))
    } else {
        // Non-embedded simple fonts are drawn by the viewer with a full system font.
        is_winansi(c)
    };
    if covered || c == ' ' && coverage.get(&s.font_name).is_some_and(|set| set.contains(&' ')) {
        return None;
    }
    let slot = style_slot(s.family(), s.bold, s.italic);
    Some(if is_winansi(c) { Fallback::Builtin(slot) } else { Fallback::System(slot) })
}

pub fn load_fallback(doc: &mut PdfDocument, f: Fallback) -> Result<PdfFontToken, String> {
    match f {
        Fallback::Builtin(i) => Ok(doc.fonts_mut().new_built_in(BUILTINS[i as usize])),
        Fallback::System(i) => {
            let path = system_font_path(i).ok_or("No system font available for these characters")?;
            doc.fonts_mut().load_true_type_from_file(&path, true).map_err(err)
        }
    }
}

// ---- Layout ----

/// What an edit did, for the UI.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditNote {
    /// Characters drawn with a substitute font because the original lacks them.
    pub substituted: String,
}

/// Font used for a run: the style's own font or a fallback.
#[derive(Clone, Copy, PartialEq)]
enum FontRef {
    Original(usize),
    Fallback(Fallback),
}

/// Assigns a style to each character of `new` by diffing against `old`:
/// unchanged prefix/suffix keep theirs, inserted text takes its neighbour's.
pub fn restyle(old: &[(char, usize)], new: &str) -> Vec<(char, usize)> {
    let new: Vec<char> = new.chars().collect();
    let pre = old.iter().zip(&new).take_while(|((a, _), b)| a == *b).count();
    let max_suf = old.len().min(new.len()) - pre;
    let suf = old.iter().rev().zip(new.iter().rev()).take(max_suf).take_while(|((a, _), b)| a == *b).count();
    let default = old.get(pre.saturating_sub(1)).or(old.get(pre)).map(|&(_, s)| s).unwrap_or(0);
    let mut out = Vec::with_capacity(new.len());
    for (i, &c) in new.iter().enumerate() {
        let s = if i < pre {
            old[i].1
        } else if i >= new.len() - suf {
            old[old.len() - (new.len() - i)].1
        } else {
            default
        };
        out.push((c, s));
    }
    out
}

struct Measurer<'d, 'a> {
    doc: &'d PdfDocument<'a>,
    cache: HashMap<(usize, u32, char), f32>,
}

impl<'d, 'a> Measurer<'d, 'a> {
    /// Horizontal advance of `c`. Measured as right edge of "c+sentinel"
    /// minus right edge of "sentinel", which cancels the glyph's side bearings.
    fn advance(&mut self, key: usize, font: &PdfFontToken, size: f32, c: char, sentinel: char) -> f32 {
        let k = (key, size.to_bits(), c);
        if let Some(&w) = self.cache.get(&k) {
            return w;
        }
        let right = |s: String| -> Option<f32> {
            let o = PdfPageTextObject::new(self.doc, s, *font, PdfPoints::new(size)).ok()?;
            Some(o.bounds().ok()?.to_rect().right().value)
        };
        let w = match (right(format!("{c}{sentinel}")), right(sentinel.to_string())) {
            (Some(a), Some(b)) => (a - b).max(0.0),
            _ => size * 0.5,
        };
        self.cache.insert(k, w);
        w
    }
}

struct Placed {
    text: String,
    font: FontRef,
    style: usize,
    x: f32,
    y: f32,
}

/// Lays `chars` out as wrapped lines and returns the runs to draw.
#[allow(clippy::too_many_arguments)]
fn lay_out(
    chars: &[(char, usize)],
    styles: &[Style],
    fonts: &[(FontRef, PdfFontToken, char)],
    font_of: &dyn Fn(char, usize) -> FontRef,
    m: &mut Measurer,
    x0: f32,
    y0: f32,
    width: Option<f32>,
    line_gap: f32,
    // Word start positions are scaled by this (<= 1) to match the original setting.
    squeeze: f32,
) -> Vec<Placed> {
    let lookup = |f: FontRef| fonts.iter().position(|(r, _, _)| *r == f).unwrap();
    let adv = |m: &mut Measurer, c: char, s: usize| {
        let fi = lookup(font_of(c, s));
        let (_, tok, sentinel) = &fonts[fi];
        m.advance(fi, tok, styles[s].size, c, *sentinel)
    };

    // Split into words (non-space runs), spaces and hard breaks, then wrap greedily.
    let mut lines: Vec<Vec<(char, usize, f32)>> = vec![Vec::new()];
    let mut line_w = 0.0;
    let mut i = 0;
    while i < chars.len() {
        let (c, s) = chars[i];
        if c == '\n' {
            lines.push(Vec::new());
            line_w = 0.0;
            i += 1;
            continue;
        }
        let mut j = i;
        let is_space = c.is_whitespace();
        while j < chars.len() && chars[j].0 != '\n' && chars[j].0.is_whitespace() == is_space {
            j += 1;
        }
        let word: Vec<(char, usize, f32)> = chars[i..j].iter().map(|&(c, s)| (c, s, adv(m, c, s))).collect();
        let w: f32 = word.iter().map(|g| g.2).sum();
        let line = lines.last_mut().unwrap();
        if is_space {
            if !line.is_empty() {
                line.extend(word);
                line_w += w;
            }
        } else if let Some(limit) = width.filter(|&l| line_w + w > l && !line.is_empty()) {
            let _ = limit;
            while line.last().is_some_and(|g| g.0.is_whitespace()) {
                line.pop();
            }
            lines.push(word);
            line_w = w;
        } else {
            line.extend(word);
            line_w += w;
        }
        let _ = s;
        i = j;
    }

    // When the original was set tighter than our advances, every glyph is
    // placed individually (as Chromium does) so spacing shrinks evenly;
    // otherwise whole words are drawn as one object.
    let per_glyph = squeeze < 0.995;
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let y = y0 - n as f32 * line_gap;
        let mut x = 0.0;
        let mut k = 0;
        while k < line.len() {
            let f = font_of(line[k].0, line[k].1);
            let s = line[k].1;
            let start_x = x;
            let mut text = String::new();
            // Leading whitespace (only possible at a hard line start) is skipped.
            if line[k].0.is_whitespace() {
                x += line[k].2;
                k += 1;
                continue;
            }
            // The unit: one glyph or one word, plus its trailing whitespace so
            // extracted text keeps word breaks.
            while k < line.len() && font_of(line[k].0, line[k].1) == f && line[k].1 == s && !line[k].0.is_whitespace() {
                text.push(line[k].0);
                x += line[k].2;
                k += 1;
                if per_glyph {
                    break;
                }
            }
            while k < line.len() && line[k].0.is_whitespace() && line[k].1 == s {
                text.push(line[k].0);
                x += line[k].2;
                k += 1;
            }
            out.push(Placed { text, font: f, style: s, x: x0 + start_x * squeeze, y });
        }
    }
    out
}

/// Replaces a block's text, keeping per-character styles and fonts where possible.
pub fn edit_text(
    doc: &mut PdfDocument,
    fonts: &mut Fonts,
    page_index: u16,
    model: &PageModel,
    block: usize,
    new_text: &str,
) -> Result<EditNote, String> {
    let b = model.blocks.get(block).ok_or("Unknown text block")?;
    let new_text = new_text.replace("\r\n", "\n");
    let chars = if new_text.is_empty() { Vec::new() } else { restyle(&b.chars, &new_text) };
    let content = (!chars.is_empty()).then_some(chars);
    // Paragraphs re-wrap at their own width. A single line grows sideways
    // (like a heading) and only wraps at a right margin mirroring the left one.
    let width = if b.lines > 1 {
        b.width * 1.02 + 1.0
    } else {
        let page_w = doc.pages().get(page_index).map_err(err)?.width().value;
        (page_w - b.x - b.x.min(page_w * 0.15)).max(b.width * 1.02 + 1.0)
    };
    let orig = (b.lines > 1).then_some((&b.chars[..], &b.line_ranges[..], &b.line_widths[..]));
    replace_objects(doc, fonts, page_index, model, &b.objects, content, false, b.x, b.baseline, Some(width), b.line_gap, orig)
}

/// Adds a new text block at a point in displayed page space.
#[allow(clippy::too_many_arguments)]
pub fn add_text(
    doc: &mut PdfDocument,
    fonts: &mut Fonts,
    page_index: u16,
    geom: &Geom,
    at: (f32, f32),
    text: &str,
    size: f32,
    family: &str,
    bold: bool,
    italic: bool,
    color: (u8, u8, u8),
) -> Result<EditNote, String> {
    let style = Style {
        font_name: String::new(),
        size,
        color: (color.0, color.1, color.2, 255),
        rep: usize::MAX,
        embedded: false,
        bold,
        italic,
        serif: family == "serif",
        mono: family == "monospace",
    };
    let model = PageModel { styles: vec![style], blocks: vec![], images: vec![] };
    let chars: Vec<(char, usize)> = text.replace("\r\n", "\n").chars().map(|c| (c, 0)).collect();
    // `at` is the top-left of the text box; the first baseline sits one ascent below.
    let (x, y) = geom.unpoint(at.0, at.1 + size * 0.8);
    replace_objects(doc, fonts, page_index, &model, &[], Some(chars), true, x, y, None, size * 1.25, None)
}

/// A block's original characters, its line ranges and their drawn widths.
type OrigLines<'b> = (&'b [(char, usize)], &'b [(usize, usize)], &'b [f32]);

/// Removes `remove` objects and draws `content` (if any) in their place.
#[allow(clippy::too_many_arguments)]
fn replace_objects(
    doc: &mut PdfDocument,
    fonts: &mut Fonts,
    page_index: u16,
    model: &PageModel,
    remove: &[usize],
    content: Option<Vec<(char, usize)>>,
    // New text (no original font to reuse): standard fonts only.
    fresh: bool,
    x0: f32,
    y0: f32,
    width: Option<f32>,
    line_gap: f32,
    // Original text and its lines: widths are measured with our own advances
    // so unchanged text wraps exactly where it did before.
    orig: Option<OrigLines>,
) -> Result<EditNote, String> {
    let mut note = EditNote::default();
    let styles = &model.styles;

    // 1. Decide fonts per character and load any fallbacks (needs &mut doc).
    let new_objects_only = fresh;
    if fonts.coverage.is_none() && !new_objects_only {
        fonts.coverage = Some(scan_coverage(doc));
    }
    let empty = HashMap::new();
    let coverage = fonts.coverage.clone().unwrap_or_default();
    let font_of = |c: char, s: usize| -> FontRef {
        if new_objects_only {
            let st = &styles[s];
            let slot = style_slot(st.family(), st.bold, st.italic);
            return FontRef::Fallback(if is_winansi(c) || c.is_whitespace() { Fallback::Builtin(slot) } else { Fallback::System(slot) });
        }
        match resolve(&styles[s], c, if styles[s].embedded { &coverage } else { &empty }) {
            None => FontRef::Original(s),
            Some(f) => FontRef::Fallback(f),
        }
    };
    let mut needed: Vec<FontRef> = Vec::new();
    if let Some(chars) = &content {
        for &(c, s) in chars {
            if c == '\n' {
                continue;
            }
            let f = font_of(c, s);
            if let FontRef::Fallback(_) = f {
                if !c.is_whitespace() && !note.substituted.contains(c) && !new_objects_only {
                    note.substituted.push(c);
                }
            }
            if !needed.contains(&f) {
                needed.push(f);
            }
        }
    }
    for f in &needed {
        if let FontRef::Fallback(fb) = f {
            if !fonts.loaded.contains_key(fb) {
                let tok = load_fallback(doc, *fb)?;
                fonts.loaded.insert(*fb, tok);
            }
        }
    }

    // 2. Build the new text objects (fonts borrowed from existing objects).
    let page = doc.pages().get(page_index).map_err(err)?;
    let mut new_objects = Vec::new();
    if let Some(chars) = &content {
        let mut table: Vec<(FontRef, PdfFontToken, char)> = Vec::new();
        for f in &needed {
            match *f {
                FontRef::Original(s) => {
                    let obj = page.objects().get(styles[s].rep).map_err(err)?;
                    let tok = obj.as_text_object().ok_or("Style object is not text")?.font().token();
                    let sentinel = coverage
                        .get(&styles[s].font_name)
                        .and_then(|set| set.iter().copied().filter(|c| c.is_alphanumeric()).min())
                        .unwrap_or('.');
                    table.push((*f, tok, sentinel));
                }
                FontRef::Fallback(fb) => table.push((*f, fonts.loaded[&fb], '.')),
            }
        }
        let mut measurer = Measurer { doc, cache: HashMap::new() };
        let mut width = width;
        // How much tighter the original was set than our advances (word spacing).
        let mut ratios = Vec::new();
        if let (Some((old, ranges, drawn)), Some(w)) = (orig, width.as_mut()) {
            for (&(a, b), &dw) in ranges.iter().zip(drawn) {
                let mut line = &old[a..b];
                while line.last().is_some_and(|c| c.0.is_whitespace()) {
                    line = &line[..line.len() - 1];
                }
                let mut lw = 0.0;
                for &(c, s) in line {
                    let f = font_of(c, s);
                    if let Some(fi) = table.iter().position(|(r, _, _)| *r == f) {
                        let (_, tok, sentinel) = &table[fi];
                        lw += measurer.advance(fi, tok, styles[s].size, c, *sentinel);
                    }
                }
                *w = w.max(lw + 0.01);
                if lw > 0.0 && b - a > 10 {
                    ratios.push(dw / lw);
                }
            }
        }
        ratios.sort_by(f32::total_cmp);
        let squeeze = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0).clamp(0.9, 1.0);
        let placed = lay_out(chars, styles, &table, &font_of, &mut measurer, x0, y0, width, line_gap, squeeze);
        for p in placed {
            let st = &styles[p.style];
            let tok = table.iter().find(|(r, _, _)| *r == p.font).unwrap().1;
            let mut obj = PdfPageTextObject::new(doc, &p.text, tok, PdfPoints::new(st.size)).map_err(err)?;
            obj.set_fill_color(PdfColor::new(st.color.0, st.color.1, st.color.2, st.color.3)).map_err(err)?;
            obj.apply_matrix(PdfMatrix::new(1.0, 0.0, 0.0, 1.0, p.x, p.y)).map_err(err)?;
            new_objects.push(obj);
        }
    }
    drop(page);

    // 3. Swap the objects on the page.
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let mut sorted = remove.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let insert_at = sorted.first().copied();
    for &i in sorted.iter().rev() {
        let removed = page.objects_mut().remove_object_at_index(i as PdfPageObjectIndex).map_err(err)?;
        std::mem::forget(removed);
    }
    for (k, obj) in new_objects.into_iter().enumerate() {
        let added = match insert_at {
            Some(at) => page.objects_mut().insert_object_at_index((at + k) as PdfPageObjectIndex, PdfPageObject::Text(obj)),
            None => page.objects_mut().add_text_object(obj),
        };
        added.map_err(err)?;
    }
    page.regenerate_content().map_err(err)?;
    Ok(note)
}

// ---- Moving, resizing, deleting ----

/// Affine map taking user-space box `from` onto `to` (both [l, b, r, t]).
fn box_map(from: [f32; 4], to: [f32; 4]) -> PdfMatrix {
    let sx = (to[2] - to[0]) / (from[2] - from[0]).max(1e-3);
    let sy = (to[3] - to[1]) / (from[3] - from[1]).max(1e-3);
    PdfMatrix::new(sx, 0.0, 0.0, sy, to[0] - from[0] * sx, to[1] - from[1] * sy)
}

/// Moves/resizes objects so that the user-space box `from` lands on `to`.
pub fn transform_objects(doc: &PdfDocument, page_index: u16, objects: &[usize], from: [f32; 4], to: [f32; 4]) -> Result<(), String> {
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let m = box_map(from, to);
    for &i in objects {
        let mut obj = page.objects().get(i as PdfPageObjectIndex).map_err(err)?;
        obj.apply_matrix(m).map_err(err)?;
    }
    page.regenerate_content().map_err(err)
}

pub fn delete_objects(doc: &PdfDocument, page_index: u16, objects: &[usize]) -> Result<(), String> {
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let mut sorted = objects.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for &i in sorted.iter().rev() {
        std::mem::forget(page.objects_mut().remove_object_at_index(i as PdfPageObjectIndex).map_err(err)?);
    }
    page.regenerate_content().map_err(err)
}

// ---- Images ----

fn load_image_object<'a>(doc: &PdfDocument<'a>, path: &str) -> Result<(PdfPageImageObject<'a>, (u32, u32)), String> {
    let lower = path.to_lowercase();
    let dims = image::image_dimensions(path).map_err(|e| format!("Cannot read image: {e}"))?;
    let obj = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        // Keep JPEG data as-is (DCT) instead of re-encoding it.
        PdfPageImageObject::new_from_jpeg_file(doc, path).map_err(err)?
    } else {
        let img = image::open(path).map_err(|e| format!("Cannot read image: {e}"))?;
        PdfPageImageObject::new(doc, &img).map_err(err)?
    };
    Ok((obj, dims))
}

/// Replaces an image, fitting the new picture inside the old one's frame
/// (same position, rotation and z-order; aspect ratio preserved).
pub fn replace_image(doc: &PdfDocument, page_index: u16, index: usize, path: &str) -> Result<(), String> {
    let (mut new_obj, (pw, ph)) = load_image_object(doc, path)?;
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let old = page.objects().get(index as PdfPageObjectIndex).map_err(err)?;
    if old.as_image_object().is_none() {
        return Err("Not an image".into());
    }
    let m = old.matrix().map_err(err)?;
    let (wl, hl) = (m.a().hypot(m.b()), m.c().hypot(m.d()));
    let (r_old, r_new) = (wl / hl.max(1e-3), pw as f32 / ph.max(1) as f32);
    let (sx, sy) = if r_new > r_old { (1.0, r_old / r_new) } else { (r_new / r_old, 1.0) };
    let fit = PdfMatrix::new(sx, 0.0, 0.0, sy, (1.0 - sx) / 2.0, (1.0 - sy) / 2.0);
    new_obj.apply_matrix(fit.multiply(m)).map_err(err)?;
    drop(old);
    page.objects_mut()
        .insert_object_at_index(index as PdfPageObjectIndex, PdfPageObject::Image(new_obj))
        .map_err(err)?;
    std::mem::forget(page.objects_mut().remove_object_at_index((index + 1) as PdfPageObjectIndex).map_err(err)?);
    page.regenerate_content().map_err(err)
}

/// Adds an image with its top-left corner at a displayed page point.
pub fn add_image(doc: &PdfDocument, page_index: u16, geom: &Geom, at: (f32, f32), path: &str) -> Result<(), String> {
    let (mut obj, (pw, ph)) = load_image_object(doc, path)?;
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    // 96 dpi, limited to half the page width.
    let (dw, _) = geom.display_size();
    let mut w = pw as f32 * 0.75;
    let mut h = ph as f32 * 0.75;
    if w > dw * 0.5 {
        h *= dw * 0.5 / w;
        w = dw * 0.5;
    }
    // Unit square corners -> displayed rect, expressed in user space.
    let p00 = geom.unpoint(at.0, at.1 + h);
    let p10 = geom.unpoint(at.0 + w, at.1 + h);
    let p01 = geom.unpoint(at.0, at.1);
    let m = PdfMatrix::new(p10.0 - p00.0, p10.1 - p00.1, p01.0 - p00.0, p01.1 - p00.1, p00.0, p00.1);
    obj.apply_matrix(m).map_err(err)?;
    page.objects_mut().add_image_object(obj).map_err(err)?;
    page.regenerate_content().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restyle_keeps_unchanged_styles() {
        let old: Vec<(char, usize)> = "ab CD ef".chars().enumerate().map(|(i, c)| (c, if (3..5).contains(&i) { 1 } else { 0 })).collect();
        let new = restyle(&old, "ab CDX ef");
        let styles: Vec<usize> = new.iter().map(|&(_, s)| s).collect();
        assert_eq!(styles, vec![0, 0, 0, 1, 1, 1, 0, 0, 0]);
        let new = restyle(&old, "Zab CD ef");
        assert_eq!(new[0].1, 0);
        assert_eq!(new[4].1, 1);
    }

    #[test]
    fn winansi() {
        assert!(is_winansi('é') && is_winansi('€') && is_winansi('A'));
        assert!(!is_winansi('ł') && !is_winansi('中'));
    }
}
