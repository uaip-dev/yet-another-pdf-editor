"""Generates a small local test corpus in test-pdfs/ (git-ignored)."""
import sys
from pathlib import Path
from reportlab.lib.pagesizes import A4, letter, landscape
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter

out = Path(sys.argv[1] if len(sys.argv) > 1 else "test-pdfs")
out.mkdir(exist_ok=True)

def multi(path, pages):
    c = canvas.Canvas(str(path), pagesize=A4)
    c.setTitle("Multi-page test document")
    for i in range(pages):
        size = landscape(A4) if i % 7 == 3 else A4
        c.setPageSize(size)
        c.setFont("Helvetica-Bold", 28)
        c.drawString(72, size[1] - 100, f"Page {i + 1}")
        c.setFont("Times-Roman", 12)
        for line in range(30):
            c.drawString(72, size[1] - 140 - line * 18, f"Line {line + 1}: The quick brown fox jumps over the lazy dog.")
        c.setFillColorRGB(0.2, 0.45, 0.85)
        c.rect(size[0] - 200, 60, 120, 80, fill=1, stroke=0)
        c.showPage()
    c.save()

multi(out / "multipage.pdf", 200)

c = canvas.Canvas(str(out / "letter.pdf"), pagesize=letter)
c.setFont("Helvetica", 18)
c.drawString(72, 700, "Single US Letter page")
c.save()

w = PdfWriter(clone_from=str(out / "letter.pdf"))
w.encrypt(user_password="test", owner_password="owner")
w.write(out / "encrypted-password-test.pdf")
print("wrote", sorted(p.name for p in out.iterdir()))
