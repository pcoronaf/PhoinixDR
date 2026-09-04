//! Small helpers for tabular and JSON output.

use std::io::Write;

/// Prints a line to stdout; if the consumer closed the pipe (`| head`), the
/// process exits quietly with success instead of panicking.
macro_rules! outln {
    () => {
        $crate::output::write_line(String::new())
    };
    ($($arg:tt)*) => {
        $crate::output::write_line(format!($($arg)*))
    };
}
pub(crate) use outln;

/// Writes `text` plus a newline to stdout, exiting on a broken pipe.
pub fn write_line(text: String) {
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .is_err()
    {
        std::process::exit(0);
    }
}

/// Writes `text` verbatim to stdout, exiting on a broken pipe.
pub fn write_raw(text: &str) {
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(text.as_bytes()).is_err() {
        std::process::exit(0);
    }
}

/// Renders rows as a left-aligned, space-padded table.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let columns = header.len();
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.chars().count());
            }
        }
    }
    let mut out = String::new();
    let render = |cells: &[String], out: &mut String| {
        let last = cells.len().saturating_sub(1);
        for (i, cell) in cells.iter().enumerate() {
            out.push_str(cell);
            if i < last {
                let width = widths.get(i).copied().unwrap_or(0);
                let pad = width.saturating_sub(cell.chars().count()) + 2;
                out.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push('\n');
    };
    let header_cells: Vec<String> = header.iter().map(|h| (*h).to_owned()).collect();
    render(&header_cells, &mut out);
    for row in rows {
        render(row, &mut out);
    }
    out
}

/// Writes `value` as pretty JSON to stdout.
pub fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

/// Renders an optional value or a dash.
pub fn opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| v.to_string())
}
