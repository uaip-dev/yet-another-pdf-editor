//! Dev tool: dumps the page objects of a PDF page.
//! cargo run --example inspect -- <file.pdf> [page-number]

use pdfium_render::prelude::*;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).ok_or("usage: inspect <file.pdf> [page]")?;
    let page_no: u16 = args.get(2).map(|p| p.parse()).transpose()?.unwrap_or(1);
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("pdfium").join(Pdfium::pdfium_platform_library_name());
    let pdfium = Pdfium::new(Pdfium::bind_to_library(lib.to_string_lossy().as_ref())?);
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let page = doc.pages().get(page_no - 1)?;
    println!("page {page_no}: {} objects", page.objects().len());
    for (i, obj) in page.objects().iter().enumerate() {
        let b = obj.bounds().map(|q| q.to_rect()).ok();
        let r = b.map(|r| format!("[{:.1},{:.1} {:.1}x{:.1}]", r.left().value, r.bottom().value, r.width().value, r.height().value)).unwrap_or_default();
        match &obj {
            PdfPageObject::Text(t) => {
                let f = t.font();
                let m = t.matrix().ok().map(|m| format!("[{:.2} {:.2} {:.2} {:.2} {:.1} {:.1}]", m.a(), m.b(), m.c(), m.d(), m.e(), m.f())).unwrap_or_default();
                println!("{i:4} TEXT  {r} size={:.1} font={} embedded={:?} data={}B m={m} {:?}",
                    t.unscaled_font_size().value, f.name(), f.is_embedded().ok(), f.data().map(|d| d.len()).unwrap_or(0), t.text());
            }
            PdfPageObject::Image(im) => println!("{i:4} IMAGE {r} {}x{}", im.width().unwrap_or(0), im.height().unwrap_or(0)),
            other => println!("{i:4} {:?} {r}", other.object_type()),
        }
    }
    Ok(())
}
