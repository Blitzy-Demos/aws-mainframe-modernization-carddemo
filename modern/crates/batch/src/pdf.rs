//! The statement PDF: the replacement of the `TXT2PDF` step of `TXT2PDF1.JCL`, which runs the
//! `%TXT2PDF` exec over `STATEMNT.PS` (CBSTM03A-051). `creastmt` streams the text records it
//! reads back from `STATEMNT.PS` (CBSTM03A-031) into `render`, one at a time, and writes the
//! returned bytes as `STATEMNT.PDF`; this module does no I/O (Local-Only Runtime).
//!
//! - Input: only the 80-byte text records, never a monetary value (R1). Page positions are whole
//!   millimetres computed with checked integer arithmetic; `mm` is the one conversion to the
//!   `f32` lengths `printpdf` takes, and no floating-point arithmetic happens here
//!   (`clippy::float_arithmetic` is denied).
//! - Layout: A4 portrait pages in the built-in `Courier` font, `LINES_PER_PAGE` records per page.
//!   Page size, margins, records per page and font size are the replacement's own and are not
//!   compared (CBSTM03A-051).
//! - One record, one `use_text` call: each call opens and closes its own text section, and the
//!   `lopdf` 0.31 `extract_text` ends the text of a section with one line break at its `ET`. Each
//!   record therefore extracts as exactly one line holding all 80 bytes, trailing spaces
//!   included, which is the comparison of CBSTM03A-052. A whitespace-only difference there is
//!   escalated, never normalised (R7), so no record is trimmed here.
//! - Bytes: every byte of a record must be printable ASCII (`0x20` to `0x7E`). The built-in fonts
//!   encode text through `WinAnsiEncoding` and silently drop what it cannot encode, so any other
//!   byte would not survive extraction: it is a typed error, never a lossy conversion (R7).
//! - Font resources: the saved document is re-read once to make each page's `/Font` resource a
//!   direct dictionary, so that `extract_text` decodes with the font's own `WinAnsiEncoding` and
//!   `'` and `` ` `` round-trip (`inline_page_fonts`, escalation candidate `X-CREASTMT-8`). The
//!   bytes re-read are only those `printpdf` produced in this process, never a PDF from outside
//!   it (E41).
//! - Errors carry a record number at most, never record text, because statement records hold
//!   customer names, addresses and balances (Cardholder Data Redaction).

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt;

use printpdf::lopdf::{self, Document, Object, ObjectId};
use printpdf::{BuiltinFont, Mm, PdfDocument};

/// Page width in whole millimetres: A4 portrait.
const PAGE_WIDTH_MM: u16 = 210;

/// Page height in whole millimetres: A4 portrait.
const PAGE_HEIGHT_MM: u16 = 297;

/// Records per page, a fixed number (CBSTM03A-051).
const LINES_PER_PAGE: u16 = 60;

/// Distance between two baselines in whole millimetres, above the 9-point font's 3.2 mm.
const LINE_HEIGHT_MM: u16 = 4;

/// Baseline of a page's first record, in millimetres from the bottom edge. The last of the
/// `LINES_PER_PAGE` records sits 59 line heights lower, at 44 mm.
const TOP_BASELINE_MM: u16 = 280;

/// Left edge of every record, in millimetres from the left edge of the page.
const LEFT_MARGIN_MM: u16 = 10;

/// Font size in points. Courier advances 0.6 em per character, so 80 columns take about 152 mm,
/// inside the 200 mm right of the margin.
const FONT_SIZE_PT: f32 = 9.0;

/// Document title, the base name of `STATEMNT.PS`.
const TITLE: &str = "STATEMNT";

/// Name of the one layer of every page.
const LAYER: &str = "STATEMNT";

/// Why `render` produced no PDF. No variant holds record text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderError {
    /// Record `line` (1-based, in `STATEMNT.PS` order) holds a byte that is not printable ASCII.
    NonText { line: usize },
    /// A checked page-layout step overflowed. No record count reaches it.
    Layout,
    /// `printpdf` could not register the font or serialise the document, or the saved document
    /// could not be re-read to inline its font resources. The library error is not kept, so
    /// nothing it might quote reaches a log.
    Pdf,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonText { line } => {
                write!(f, "statement PDF: record {line} is not printable ASCII")
            }
            Self::Layout => f.write_str("statement PDF: page layout overflow"),
            Self::Pdf => f.write_str("statement PDF: rendering failed"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Renders the `STATEMNT.PS` records as `STATEMNT.PDF`: one `use_text` call per record with all
/// 80 bytes, in record order, `LINES_PER_PAGE` records per page (CBSTM03A-051), so that the
/// extracted text equals the records line for line (CBSTM03A-052). No records give one empty
/// page.
///
/// `lines` is any sequence of records, borrowed or owned, taken one at a time: `creastmt`
/// streams the records it reads back from `STATEMNT.PS` without holding them all, and the
/// bytes rendered are the same whatever the sequence's type.
///
/// # Errors
///
/// - [`RenderError::NonText`] when a record holds a byte outside printable ASCII.
/// - [`RenderError::Layout`] when a checked layout step overflows.
/// - [`RenderError::Pdf`] when `printpdf` cannot register the font or serialise the document, or
///   the saved document cannot be re-read and written again with its font resources inlined.
pub(crate) fn render<I>(lines: I) -> Result<Vec<u8>, RenderError>
where
    I: IntoIterator,
    I::Item: Borrow<[u8; 80]>,
{
    let (doc, first_page, first_layer) =
        PdfDocument::new(TITLE, mm(PAGE_WIDTH_MM), mm(PAGE_HEIGHT_MM), LAYER);
    let font = doc
        .add_builtin_font(BuiltinFont::Courier)
        .map_err(|_| RenderError::Pdf)?;
    let mut layer = doc.get_page(first_page).get_layer(first_layer);
    for (index, record) in lines.into_iter().enumerate() {
        let text = record_text(index, record.borrow())?;
        let (new_page, baseline) = place(index)?;
        if new_page {
            let (page, page_layer) = doc.add_page(mm(PAGE_WIDTH_MM), mm(PAGE_HEIGHT_MM), LAYER);
            layer = doc.get_page(page).get_layer(page_layer);
        }
        layer.use_text(text, FONT_SIZE_PT, mm(LEFT_MARGIN_MM), mm(baseline), &font);
    }
    let saved = doc.save_to_bytes().map_err(|_| RenderError::Pdf)?;
    inline_page_fonts(&saved).map_err(|_| RenderError::Pdf)
}

/// The PDF `bytes` with the `/Font` entry of every page's resources replaced by the dictionary
/// it refers to.
///
/// `printpdf` 0.7 always writes that entry as an indirect reference, and the `lopdf` 0.31
/// `extract_text` collects a page's fonts only from a direct `/Font` dictionary. Without one it
/// decodes every string with `StandardEncoding` instead of the `WinAnsiEncoding` the font
/// declares, so `'` and `` ` `` come back as typographic quotes and the comparison of
/// CBSTM03A-052 fails for any statement that holds them, such as a customer named with an
/// apostrophe. The inlined dictionary is a copy of the one referred to, so every page renders as
/// before.
fn inline_page_fonts(bytes: &[u8]) -> lopdf::Result<Vec<u8>> {
    let mut doc = Document::load_mem(bytes)?;
    // Pages that share one resource dictionary are rewritten once.
    let resources = doc
        .get_pages()
        .values()
        .map(|&page| doc.get_dictionary(page)?.get(b"Resources")?.as_reference())
        .collect::<lopdf::Result<BTreeSet<ObjectId>>>()?;
    for id in resources {
        let fonts_id = doc.get_dictionary(id)?.get(b"Font")?.as_reference()?;
        let fonts = doc.get_dictionary(fonts_id)?.clone();
        doc.get_dictionary_mut(id)?
            .set("Font", Object::Dictionary(fonts));
    }
    let mut pdf = Vec::new();
    doc.save_to(&mut pdf)?;
    Ok(pdf)
}

/// The record at `index` as text, all 80 bytes, when every byte is printable ASCII.
fn record_text(index: usize, record: &[u8; 80]) -> Result<&str, RenderError> {
    match std::str::from_utf8(record) {
        Ok(text) if text.bytes().all(|byte| matches!(byte, b' '..=b'~')) => Ok(text),
        _ => Err(RenderError::NonText {
            line: index.checked_add(1).ok_or(RenderError::Layout)?,
        }),
    }
}

/// Where the record at `index` goes: whether it opens a page after the first, and its baseline
/// in whole millimetres from the bottom edge.
fn place(index: usize) -> Result<(bool, u16), RenderError> {
    let row =
        u16::try_from(index % usize::from(LINES_PER_PAGE)).map_err(|_| RenderError::Layout)?;
    let baseline = row
        .checked_mul(LINE_HEIGHT_MM)
        .and_then(|offset| TOP_BASELINE_MM.checked_sub(offset))
        .ok_or(RenderError::Layout)?;
    Ok((row == 0 && index > 0, baseline))
}

/// Whole millimetres as the `printpdf` length; `f32::from` is exact for every `u16`.
fn mm(value: u16) -> Mm {
    Mm(f32::from(value))
}

#[cfg(test)]
mod tests {
    use printpdf::lopdf::Document;

    use super::{LINES_PER_PAGE, RenderError, place, record_text, render};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A record holding `content`, then spaces to 80 bytes.
    fn record(content: &[u8]) -> [u8; 80] {
        let mut record = [b' '; 80];
        for (slot, byte) in record.iter_mut().zip(content) {
            *slot = *byte;
        }
        record
    }

    /// The lines `extract_text` yields over every page of the PDF `bytes`, in page order, and
    /// the page count.
    fn extracted(bytes: &[u8]) -> Result<(Vec<String>, usize), Box<dyn std::error::Error>> {
        let doc = Document::load_mem(bytes)?;
        let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
        let text = doc.extract_text(&pages)?;
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        // Every text section ends with a line break, so the split leaves one empty last piece.
        if lines.pop().as_deref() != Some("") {
            return Err("the extracted text does not end with a line break".into());
        }
        Ok((lines, pages.len()))
    }

    /// CBSTM03A-052: 130 distinct records over three pages, which together hold every printable
    /// ASCII byte, `'` and `` ` `` included, and each end in 16 spaces, extract as the same lines
    /// in the same order, whether they are passed as a slice or streamed as owned records.
    #[test]
    fn records_extract_as_their_lines_in_order() -> TestResult {
        let printable: Vec<u8> = (b' '..=b'~').collect();
        let records: Vec<[u8; 80]> = (0..130_usize)
            .map(|n| {
                let mut content = format!("{n:03} ").into_bytes();
                content.extend(printable.iter().cycle().skip(n).take(60));
                record(&content)
            })
            .collect();
        let expected = records
            .iter()
            .map(|line| std::str::from_utf8(line))
            .collect::<Result<Vec<&str>, _>>()?;
        for bytes in [render(&records)?, render(records.iter().copied())?] {
            let (lines, pages) = extracted(&bytes)?;
            assert_eq!(lines, expected);
            assert_eq!(pages, 3);
        }
        Ok(())
    }

    /// CBSTM03A-052: trailing spaces are kept, so a blank record extracts as 80 spaces.
    #[test]
    fn blank_record_extracts_as_80_spaces() -> TestResult {
        let (lines, pages) = extracted(&render([[b' '; 80]])?)?;
        assert_eq!(lines, [" ".repeat(80)]);
        assert_eq!(pages, 1);
        Ok(())
    }

    /// CBSTM03A-051: an empty `STATEMNT.PS` gives a PDF of one empty page.
    #[test]
    fn no_records_give_one_empty_page() -> TestResult {
        let (lines, pages) = extracted(&render(std::iter::empty::<[u8; 80]>())?)?;
        assert!(lines.is_empty());
        assert_eq!(pages, 1);
        Ok(())
    }

    /// A control byte, and valid UTF-8 outside ASCII, are refused with the 1-based record
    /// number, and the message quotes nothing of the record.
    #[test]
    fn non_text_records_are_refused_by_number() -> TestResult {
        let control = record(b"~~~~~~~~\0");
        let accented = record("\u{e9}^^^^^^^".as_bytes());
        let fine = record(b"STATEMENT LINE");
        let cases = [
            (vec![fine, fine, control], 3, '~', '\u{0}'),
            (vec![accented, fine], 1, '^', '\u{e9}'),
        ];
        for (records, line, filler, foreign) in cases {
            let error = render(&records)
                .err()
                .ok_or("a non-text record was rendered")?;
            assert_eq!(error, RenderError::NonText { line });
            let message = error.to_string();
            assert_eq!(
                message,
                format!("statement PDF: record {line} is not printable ASCII")
            );
            assert!(!message.contains(filler) && !message.contains(foreign));
        }
        Ok(())
    }

    /// The record number of an index past the last `usize` overflows into a layout error.
    #[test]
    fn unnumbered_record_is_a_layout_error() {
        let delete = record(&[0x7F]);
        assert_eq!(record_text(usize::MAX, &delete), Err(RenderError::Layout));
        assert_eq!(
            record_text(usize::MAX, &record(b"OK")).map(str::len),
            Ok(80)
        );
    }

    /// Baselines step down by the line height and every `LINES_PER_PAGE` records open a page.
    #[test]
    fn records_are_placed_by_page_and_row() {
        let per_page = usize::from(LINES_PER_PAGE);
        let cases = [
            (0, Ok((false, 280))),
            (1, Ok((false, 276))),
            (per_page - 1, Ok((false, 44))),
            (per_page, Ok((true, 280))),
            (2 * per_page + 1, Ok((false, 276))),
            (1000 * per_page + 15, Ok((false, 220))),
        ];
        for (index, expected) in cases {
            assert_eq!(place(index), expected, "record index {index}");
        }
    }

    /// Each message names the failure, and a record by its number only.
    #[test]
    fn errors_display_without_record_text() {
        let cases = [
            (
                RenderError::NonText { line: 7 },
                "statement PDF: record 7 is not printable ASCII",
            ),
            (RenderError::Layout, "statement PDF: page layout overflow"),
            (RenderError::Pdf, "statement PDF: rendering failed"),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }
}
