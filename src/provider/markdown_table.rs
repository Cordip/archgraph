//! Strict, narrow Markdown-table compatibility boundary, not a Markdown engine.
use crate::error::compatibility;
use anyhow::Result;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}
impl Table {
    pub fn column(&self, header: &str) -> Result<usize> {
        self.headers
            .iter()
            .position(|value| value == header)
            .ok_or_else(|| compatibility(format!("required column `{header}` is missing")))
    }
    pub fn optional_column(&self, header: &str) -> Option<usize> {
        self.headers.iter().position(|value| value == header)
    }
}

fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut last_was_separator = false;
    while let Some(c) = chars.next() {
        last_was_separator = false;
        match c {
            '\\' => match chars.peek().copied() {
                Some('|') | Some('\\') => {
                    current.push(chars.next().expect("peeked character exists"));
                }
                _ => current.push('\\'),
            },
            '|' => {
                cells.push(current.trim().to_owned());
                current.clear();
                last_was_separator = true;
            }
            _ => current.push(c),
        }
    }
    cells.push(current.trim().to_owned());
    if line.starts_with('|') {
        cells.remove(0);
    }
    if last_was_separator {
        cells.pop();
    }
    cells
}

pub fn parse(markdown: &str) -> Result<Table> {
    let text = markdown.trim();
    let lines: Vec<_> = text.lines().collect();
    if lines.len() < 2 {
        return Err(compatibility(
            "table requires a header row and separator row (including for zero rows)",
        ));
    }
    let headers = cells(lines[0]);
    if headers.is_empty() || headers.iter().any(|h| h.is_empty()) {
        return Err(compatibility("table headers must be nonempty"));
    }
    let unique: BTreeSet<_> = headers.iter().collect();
    if unique.len() != headers.len() {
        return Err(compatibility("duplicate table column headers"));
    }
    let separators = cells(lines[1]);
    if separators.len() != headers.len()
        || separators.iter().any(|cell| {
            let without_left = cell.strip_prefix(':').unwrap_or(cell);
            let dashes = without_left.strip_suffix(':').unwrap_or(without_left);
            dashes.len() < 3 || !dashes.bytes().all(|b| b == b'-')
        })
    {
        return Err(compatibility(
            "invalid Markdown separator row; expected one --- cell per header",
        ));
    }
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(2) {
        let row = cells(line);
        if line.trim().is_empty() || row.len() != headers.len() {
            return Err(compatibility(format!(
                "table row {} has {} cells; expected {}",
                index + 1,
                row.len(),
                headers.len()
            )));
        }
        rows.push(row);
    }
    Ok(Table { headers, rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_pipes_and_header_lookup() {
        let table = parse("| reason | target | source |\n| :--- | ---: | :---: |\n| import a\\|b | src/b.rs | src/a.rs |\n").unwrap();
        assert_eq!(table.column("source").unwrap(), 2);
        assert_eq!(table.rows[0][0], "import a|b");
    }
    #[test]
    fn preserves_windows_separators() {
        let table = parse("source | target\n--- | ---\nsrc\\a.rs | src\\b.rs").unwrap();
        assert_eq!(table.rows[0][0], r"src\a.rs");
    }
    #[test]
    fn supports_empty_table_but_not_missing_table() {
        assert!(parse("| source | target |\n| --- | --- |\n")
            .unwrap()
            .rows
            .is_empty());
        assert!(parse("").is_err());
        assert!(parse("no results").is_err());
    }
    #[test]
    fn rejects_malformed_rows_headers_and_separator() {
        for table in [
            "| source | target |\n| --- | --- |\n| only-one |",
            "| source | target |\n| --- | --- |\n| a | b | c |",
            "| source | source |\n| --- | --- |",
            "| source | target |\n| not | separator |",
            "| source | target |\n| --- | --- |\n\n| a | b |",
        ] {
            assert!(parse(table).is_err(), "accepted {table:?}");
        }
    }
    #[test]
    fn escaped_trailing_pipe_is_not_a_border() {
        assert_eq!(
            parse("source | target\n--- | ---\na | b\\|").unwrap().rows[0][1],
            "b|"
        );
    }
}
