//! Statement PDF: the role `TXT2PDF1.JCL` plays for `STATEMNT.PS`.
//!
//! The mainframe converts the 80-byte STMTFILE records [app/cbl/CBSTM03A.CBL:L44-45] into
//! `STATEMNT.PS.PDF` with a `%TXT2PDF` exec that is not in the repository
//! [app/jcl/TXT2PDF1.JCL:L24-40], so only the PDF's text is compared, never its layout.
//!
//! The statement test extracts the text with `lopdf` 0.31, which collects the `Tj` strings of a
//! page and ends a line at every `ET`. Each record is therefore written as its own text object
//! holding the whole record, trailing spaces included, so every record extracts as exactly one
//! line with nothing trimmed.
//!
//! This module receives formatted text lines only, never a money or points value. That is why it
//! is the one place the workspace float ban admits `f32`: printpdf takes page geometry in
//! millimetres and the font size in points as `f32`.

use printpdf::{BuiltinFont, Mm, PdfDocument};

/// Document title written into the PDF information dictionary.
const DOCUMENT_TITLE: &str = "CardDemo statement";

/// Name of the single layer each page carries.
const LAYER_NAME: &str = "statement";

/// Page width: A4.
const PAGE_WIDTH_MM: f32 = 210.0;

/// Page height: A4.
const PAGE_HEIGHT_MM: f32 = 297.0;

/// Courier size in points. A Courier glyph is 0.6 em wide, so at 9 pt an 80-character record
/// spans 80 × 5.4 pt = 152.4 mm.
const FONT_SIZE_PT: f32 = 9.0;

/// Left edge of every record. With the 152.4 mm record width this leaves 32.6 mm on the right.
const LEFT_MM: f32 = 25.0;

/// Baseline of a page's first record, measured from the bottom edge.
const TOP_MM: f32 = 280.0;

/// Distance between consecutive baselines.
const LINE_HEIGHT_MM: f32 = 4.4;

/// Records per page. The last baseline sits at 280 − 59 × 4.4 = 20.4 mm above the bottom edge.
const LINES_PER_PAGE: u16 = 60;

/// Failure to produce the statement PDF.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {
    /// printpdf could not register the builtin font or serialize the document.
    #[error("statement PDF rendering failed: {0}")]
    Pdf(#[from] printpdf::Error),
}

/// Renders STMTFILE records as a PDF in builtin Courier, one text object per record,
/// [`LINES_PER_PAGE`] records to an A4 page.
///
/// Each record is passed to printpdf unchanged: never split, trimmed or padded. printpdf encodes
/// builtin-font text as `WinAnsiEncoding`, which covers the single-byte ASCII text CBSTM03A
/// writes. A record keeps its own extracted line only while it is non-empty and holds no line
/// break; fixed 80-byte STMTFILE records satisfy both. An empty slice yields a document with one
/// empty page.
///
/// # Errors
///
/// Returns [`RenderError::Pdf`] when printpdf cannot register the builtin font or serialize the
/// document.
pub(crate) fn render_statement(lines: &[String]) -> Result<Vec<u8>, RenderError> {
    let (doc, first_page, first_layer) = PdfDocument::new(
        DOCUMENT_TITLE,
        Mm(PAGE_WIDTH_MM),
        Mm(PAGE_HEIGHT_MM),
        LAYER_NAME,
    );
    let font = doc.add_builtin_font(BuiltinFont::Courier)?;

    for (page_number, page_lines) in lines.chunks(usize::from(LINES_PER_PAGE)).enumerate() {
        let (page, layer) = if page_number == 0 {
            (first_page, first_layer)
        } else {
            doc.add_page(Mm(PAGE_WIDTH_MM), Mm(PAGE_HEIGHT_MM), LAYER_NAME)
        };
        let layer = doc.get_page(page).get_layer(layer);
        // A chunk never exceeds LINES_PER_PAGE records, so the u16 row index cannot overflow.
        for (row, line) in (0_u16..).zip(page_lines) {
            let y = TOP_MM - f32::from(row) * LINE_HEIGHT_MM;
            layer.use_text(line.as_str(), FONT_SIZE_PT, Mm(LEFT_MM), Mm(y), &font);
        }
    }

    Ok(doc.save_to_bytes()?)
}

#[cfg(test)]
mod tests {
    use super::{LINES_PER_PAGE, render_statement};

    /// Parses a rendered PDF and returns its page count and the extracted text split into lines,
    /// as the statement test reads it.
    fn extract(pdf: &[u8]) -> (usize, Vec<String>) {
        let doc = lopdf::Document::load_mem(pdf).expect("rendered PDF parses");
        let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
        let text = doc
            .extract_text(&page_numbers)
            .expect("rendered PDF text extracts");
        (
            page_numbers.len(),
            text.lines().map(str::to_owned).collect(),
        )
    }

    #[test]
    fn records_extract_as_untrimmed_lines() {
        let lines = vec![
            // CBSTM03A ST-LINE0.
            format!("{0}START OF STATEMENT{0}", "*".repeat(31)),
            // CBSTM03A ST-LINE13.
            format!(
                "{:<16}{:<51}{:>13}",
                "Tran ID", "Tran Details", "Tran Amount"
            ),
            format!("{}{}", "-".repeat(70), " ".repeat(10)),
        ];
        assert!(lines.iter().all(|line| line.len() == 80));
        assert_eq!(lines[2].len() - lines[2].trim_end().len(), 10);

        let (pages, extracted) = extract(&render_statement(&lines).expect("renders"));

        assert_eq!(pages, 1);
        assert_eq!(extracted, lines);
    }

    #[test]
    fn records_past_a_full_page_continue_in_order_on_the_next() {
        let lines: Vec<String> = (0..=LINES_PER_PAGE)
            .map(|n| {
                if n == 30 {
                    " ".repeat(80)
                } else {
                    format!("STATEMENT RECORD {n:03}{:60}", "")
                }
            })
            .collect();
        assert_eq!(lines.len(), usize::from(LINES_PER_PAGE) + 1);
        assert!(lines.iter().all(|line| line.len() == 80));

        let (pages, extracted) = extract(&render_statement(&lines).expect("renders"));

        assert_eq!(pages, 2);
        assert_eq!(extracted, lines);
    }

    #[test]
    fn empty_input_renders_one_empty_page() {
        let (pages, extracted) = extract(&render_statement(&[]).expect("renders"));

        assert_eq!(pages, 1);
        assert!(extracted.is_empty());
    }
}
