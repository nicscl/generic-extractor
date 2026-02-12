//! Tabular data parsing for CSV, Excel (.xlsx/.xls/.xlsm), and OCR markdown tables.

use crate::ocr::OcrResult;
use anyhow::{Context, Result};
use calamine::{open_workbook_from_rs, Data, Reader, Xlsx, Xlsb};
use std::io::Cursor;

/// Source type of the parsed data.
#[derive(Debug, Clone)]
pub enum SourceType {
    Csv,
    Excel,
    OcrMarkdown,
}

/// Raw parsed sheet data before LLM schema discovery.
#[derive(Debug, Clone)]
pub struct RawSheet {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub source_type: SourceType,
}

/// Dispatch file parsing by extension.
pub fn parse_file(filename: &str, data: &[u8]) -> Result<Vec<RawSheet>> {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "csv" => parse_csv(filename, data),
        "xlsx" | "xlsm" => parse_excel_xlsx(data),
        "xlsb" => parse_excel_xlsb(data),
        _ => anyhow::bail!(
            "Unsupported file type: .{}. Supported: .csv, .xlsx, .xlsm, .xlsb",
            ext
        ),
    }
}

/// Parse a CSV file into a single RawSheet.
fn parse_csv(filename: &str, data: &[u8]) -> Result<Vec<RawSheet>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .from_reader(data);

    let headers: Vec<String> = reader
        .headers()
        .context("Failed to read CSV headers")?
        .iter()
        .map(|h| h.to_string())
        .collect();

    if headers.is_empty() {
        anyhow::bail!("CSV file has no headers");
    }

    let mut rows = Vec::new();
    for result in reader.records() {
        let record = result.context("Failed to read CSV record")?;
        let row: Vec<String> = record.iter().map(|f| f.to_string()).collect();
        rows.push(row);
    }

    let name = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        .rsplit('\\')
        .next()
        .unwrap_or(filename)
        .trim_end_matches(".csv")
        .to_string();

    Ok(vec![RawSheet {
        name,
        headers,
        rows,
        source_type: SourceType::Csv,
    }])
}

/// Parse an xlsx/xlsm file. All worksheets become separate RawSheet entries.
/// First row of each sheet is treated as headers.
fn parse_excel_xlsx(data: &[u8]) -> Result<Vec<RawSheet>> {
    let cursor = Cursor::new(data);
    let mut workbook: Xlsx<_> =
        open_workbook_from_rs(cursor).context("Failed to open Excel workbook")?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut sheets = Vec::new();

    for name in &sheet_names {
        let range = match workbook.worksheet_range(name) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Skipping sheet '{}': {}", name, e);
                continue;
            }
        };

        if let Some(sheet) = range_to_raw_sheet(name, &range) {
            sheets.push(sheet);
        }
    }

    if sheets.is_empty() {
        anyhow::bail!("No sheets with data found in workbook");
    }

    Ok(sheets)
}

/// Parse an xlsb file.
fn parse_excel_xlsb(data: &[u8]) -> Result<Vec<RawSheet>> {
    let cursor = Cursor::new(data);
    let mut workbook: Xlsb<_> =
        open_workbook_from_rs(cursor).context("Failed to open Excel workbook")?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut sheets = Vec::new();

    for name in &sheet_names {
        let range = match workbook.worksheet_range(name) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Skipping sheet '{}': {}", name, e);
                continue;
            }
        };

        if let Some(sheet) = range_to_raw_sheet(name, &range) {
            sheets.push(sheet);
        }
    }

    if sheets.is_empty() {
        anyhow::bail!("No sheets with data found in workbook");
    }

    Ok(sheets)
}

/// Convert a calamine Range into a RawSheet. First row = headers.
/// Skips sheets that are empty or have only a header row.
fn range_to_raw_sheet(name: &str, range: &calamine::Range<Data>) -> Option<RawSheet> {
    let mut row_iter = range.rows();

    // First row = headers
    let header_row = row_iter.next()?;
    let headers: Vec<String> = header_row.iter().map(|c| cell_to_string(c)).collect();

    // Skip sheets with no real headers
    if headers.is_empty() || headers.iter().all(|h| h.is_empty()) {
        return None;
    }

    let mut rows = Vec::new();
    for row in row_iter {
        let values: Vec<String> = row.iter().map(|c| cell_to_string(c)).collect();
        // Skip completely empty rows
        if values.iter().all(|v| v.is_empty()) {
            continue;
        }
        rows.push(values);
    }

    // Skip sheets with headers but no data rows
    if rows.is_empty() {
        return None;
    }

    Some(RawSheet {
        name: name.to_string(),
        headers,
        rows,
        source_type: SourceType::Excel,
    })
}

/// Convert a calamine cell to a string representation.
fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            // Avoid trailing ".0" for whole numbers
            if *f == (*f as i64) as f64 && f.abs() < i64::MAX as f64 {
                format!("{}", *f as i64)
            } else {
                format!("{}", f)
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => {
            // calamine DateTime — convert from Excel serial number
            excel_serial_to_string(dt.as_f64())
        }
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{:?}", e),
    }
}

/// Convert an Excel serial date number to a human-readable string.
/// Excel epoch: 1899-12-30 (with the 1900 leap year bug — day 60 is "Feb 29, 1900" which doesn't exist).
fn excel_serial_to_string(serial: f64) -> String {
    let days = serial as i64;
    let frac = serial - days as f64;

    // Adjust for Excel's 1900 leap year bug (serial > 59 means after fake Feb 29, 1900)
    let adjusted_days = if days > 59 { days - 1 } else { days };

    let base = 25569i64; // days from 1899-12-30 to 1970-01-01
    let unix_days = adjusted_days - base;
    let total_secs = unix_days * 86400 + (frac * 86400.0) as i64;

    let days_since_epoch = total_secs / 86400;
    let time_of_day = (total_secs % 86400 + 86400) % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let mut year = 1970i32;
    let mut remaining = days_since_epoch as i32;

    if remaining >= 0 {
        loop {
            let diy = if is_leap(year) { 366 } else { 365 };
            if remaining < diy {
                break;
            }
            remaining -= diy;
            year += 1;
        }
    } else {
        loop {
            year -= 1;
            let diy = if is_leap(year) { 366 } else { 365 };
            remaining += diy;
            if remaining >= 0 {
                break;
            }
        }
    }

    let dim: [i32; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for d in dim {
        if remaining < d {
            break;
        }
        remaining -= d;
        month += 1;
    }
    let day = remaining + 1;

    if hours == 0 && minutes == 0 && seconds == 0 {
        format!("{:04}-{:02}-{:02}", year, month, day)
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            year, month, day, hours, minutes, seconds
        )
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ============================================================================
// OCR markdown table parsing
// ============================================================================

/// Extract pipe-delimited markdown tables from OCR output.
/// Parses the full concatenated markdown (ocr.markdown) since tables may span
/// across page boundaries and per-page text may not contain pipe-table formatting.
///
/// Tables with identical column counts are merged into a single RawSheet
/// (common in multi-page documents like bank statements where the same table
/// continues across pages).
pub fn parse_ocr_markdown(ocr: &OcrResult) -> Result<Vec<RawSheet>> {
    let tables = extract_markdown_tables(&ocr.markdown);
    tracing::info!(
        "Found {} raw table block(s) in OCR markdown ({} chars)",
        tables.len(),
        ocr.markdown.len()
    );

    // Parse each block into a RawSheet
    let mut parsed: Vec<RawSheet> = Vec::new();
    for (idx, table) in tables.into_iter().enumerate() {
        if let Some(sheet) =
            markdown_table_to_raw_sheet(&format!("table_{}", idx + 1), &table)
        {
            parsed.push(sheet);
        }
    }

    if parsed.is_empty() {
        anyhow::bail!(
            "No tables found in OCR output ({} pages, {} chars of markdown)",
            ocr.total_pages,
            ocr.markdown.len()
        );
    }

    // Merge tables with the same column count — they're likely continuations
    // of the same table split across pages (e.g., bank statement transactions).
    let sheets = merge_similar_tables(parsed);

    tracing::info!(
        "Extracted {} table(s) from OCR markdown ({} pages)",
        sheets.len(),
        ocr.total_pages
    );

    Ok(sheets)
}

/// Merge consecutive RawSheets with the same column count.
/// In multi-page PDFs, the same table often gets split into multiple blocks
/// (one per page). We merge them so the LLM sees one coherent dataset.
fn merge_similar_tables(tables: Vec<RawSheet>) -> Vec<RawSheet> {
    if tables.is_empty() {
        return tables;
    }

    // Group by column count — tables with same number of columns are likely the same table
    let mut groups: Vec<Vec<RawSheet>> = Vec::new();
    for sheet in tables {
        let col_count = sheet.headers.len();
        if let Some(group) = groups.iter_mut().find(|g| g[0].headers.len() == col_count) {
            group.push(sheet);
        } else {
            groups.push(vec![sheet]);
        }
    }

    groups
        .into_iter()
        .map(|group| {
            if group.len() == 1 {
                return group.into_iter().next().unwrap();
            }

            // Merge: keep headers from the first sheet, concatenate all rows
            let mut iter = group.into_iter();
            let mut merged = iter.next().unwrap();
            let part_count = 1 + iter.len();
            for other in iter {
                // The "headers" of continuation blocks are actually data rows
                // (since the original table has no repeated header), so include them
                merged.rows.push(other.headers);
                merged.rows.extend(other.rows);
            }
            merged.name = format!("{} ({} parts merged)", merged.name, part_count);
            merged
        })
        .collect()
}

/// Extract contiguous blocks of pipe-delimited rows from markdown text.
/// A table block is a sequence of lines that start and end with `|`.
fn extract_markdown_tables(text: &str) -> Vec<Vec<String>> {
    let mut tables: Vec<Vec<String>> = Vec::new();
    let mut current_table: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            current_table.push(trimmed.to_string());
        } else {
            if current_table.len() >= 2 {
                // Need at least header + one data row (separator row will be filtered)
                tables.push(std::mem::take(&mut current_table));
            } else {
                current_table.clear();
            }
        }
    }
    // Don't forget the last block
    if current_table.len() >= 2 {
        tables.push(current_table);
    }

    tables
}

/// Convert a block of pipe-delimited markdown lines into a RawSheet.
/// Skips separator rows (lines like `|---|---|---| `).
fn markdown_table_to_raw_sheet(name: &str, lines: &[String]) -> Option<RawSheet> {
    let mut data_rows: Vec<Vec<String>> = Vec::new();

    for line in lines {
        // Skip separator rows
        let inner = line.trim_start_matches('|').trim_end_matches('|');
        if inner.chars().all(|c| c == '-' || c == '|' || c == ':' || c == ' ') {
            continue;
        }

        let cells: Vec<String> = inner
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        data_rows.push(cells);
    }

    if data_rows.len() < 2 {
        return None; // Need header + at least one data row
    }

    let headers = data_rows.remove(0);

    // Skip if all headers are empty
    if headers.iter().all(|h| h.is_empty()) {
        return None;
    }

    // Filter out completely empty rows
    data_rows.retain(|row| !row.iter().all(|c| c.is_empty()));

    if data_rows.is_empty() {
        return None;
    }

    Some(RawSheet {
        name: name.to_string(),
        headers,
        rows: data_rows,
        source_type: SourceType::OcrMarkdown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csv_basic() {
        let csv_data = b"name,age,city\nAlice,30,SP\nBob,25,RJ\n";
        let sheets = parse_file("test.csv", csv_data).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].headers, vec!["name", "age", "city"]);
        assert_eq!(sheets[0].rows.len(), 2);
        assert_eq!(sheets[0].rows[0], vec!["Alice", "30", "SP"]);
        assert_eq!(sheets[0].name, "test");
    }

    #[test]
    fn test_parse_csv_flexible() {
        // Rows with different column counts should still parse
        let csv_data = b"a,b,c\n1,2,3\n4,5\n";
        let sheets = parse_file("flex.csv", csv_data).unwrap();
        assert_eq!(sheets[0].rows.len(), 2);
        assert_eq!(sheets[0].rows[1], vec!["4", "5"]);
    }

    #[test]
    fn test_unsupported_extension() {
        let result = parse_file("test.txt", b"data");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_markdown_tables() {
        let md = "Some text\n\n| Name | Age |\n|------|-----|\n| Alice | 30 |\n| Bob | 25 |\n\nMore text\n";
        let tables = extract_markdown_tables(md);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].len(), 4); // header + separator + 2 data rows
    }

    #[test]
    fn test_markdown_table_to_raw_sheet() {
        let lines = vec![
            "| Name | Age | City |".to_string(),
            "|------|-----|------|".to_string(),
            "| Alice | 30 | SP |".to_string(),
            "| Bob | 25 | RJ |".to_string(),
        ];
        let sheet = markdown_table_to_raw_sheet("test", &lines).unwrap();
        assert_eq!(sheet.headers, vec!["Name", "Age", "City"]);
        assert_eq!(sheet.rows.len(), 2);
        assert_eq!(sheet.rows[0], vec!["Alice", "30", "SP"]);
    }
}
