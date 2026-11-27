use std::collections::VecDeque;

use serde::Serialize;

const MAX_BYTES: usize = 64 * 1024;
const MAX_LINES: usize = 200;
const LINE_BYTES: usize = 2048;

#[derive(Serialize)]
pub struct TailPage {
    pub cursor: u64,
    pub lines: Vec<String>,
    pub skipped: bool,
}

#[derive(Default)]
pub(super) struct TailBuffer {
    lines: VecDeque<String>,
    bytes: usize,
    cursor: u64,
}

impl TailBuffer {
    pub(super) fn append(&mut self, text: &str) {
        let count = text.lines().count();
        self.cursor += count as u64;
        for line in text.lines().skip(count.saturating_sub(MAX_LINES)) {
            let line = if line.len() > LINE_BYTES {
                let end = line.floor_char_boundary(LINE_BYTES - '…'.len_utf8());
                format!("{}…", &line[..end])
            } else {
                line.to_owned()
            };
            self.bytes += line.len() + 1;
            self.lines.push_back(line);
            while self.bytes > MAX_BYTES || self.lines.len() > MAX_LINES {
                if let Some(old) = self.lines.pop_front() {
                    self.bytes -= old.len() + 1;
                }
            }
        }
    }

    pub(super) fn since(&self, after: Option<u64>) -> TailPage {
        let first = self.cursor - self.lines.len() as u64;
        let after = after.unwrap_or(first);
        let skipped = !(first..=self.cursor).contains(&after);
        let offset = if skipped { 0 } else { (after - first) as usize };
        TailPage {
            cursor: self.cursor,
            lines: self.lines.range(offset..).cloned().collect(),
            skipped,
        }
    }
}
