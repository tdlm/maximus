use unicode_width::UnicodeWidthChar;

/// Columns between tab stops.
pub const TAB: usize = 4;
const UNDO_LIMIT: usize = 200;

/// A place in the buffer. `col` is a byte offset into the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Kinds of edit, so a run of typing or deleting undoes as one step.
#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Type,
    Delete,
    Other,
}

struct Snapshot {
    lines: Vec<String>,
    cur: Pos,
    version: u64,
}

/// Width of `ch` drawn at display column `at`; tabs reach the next tab stop.
pub fn char_width(ch: char, at: usize) -> usize {
    if ch == '\t' {
        TAB - at % TAB
    } else {
        ch.width().unwrap_or(0)
    }
}

/// Display column of byte offset `col` in `line`.
pub fn display_col(line: &str, col: usize) -> usize {
    let mut w = 0;
    for (i, ch) in line.char_indices() {
        if i >= col {
            break;
        }
        w += char_width(ch, w);
    }
    w
}

/// Byte offset of the character covering display column `target`, or the line's end.
pub fn col_at(line: &str, target: usize) -> usize {
    let mut w = 0;
    for (i, ch) in line.char_indices() {
        let cw = char_width(ch, w);
        if w + cw > target {
            return i;
        }
        w += cw;
    }
    line.len()
}

/// Characters a wrapped line may break after or before, as VS Code's defaults.
const BREAK_AFTER: &str = " \t})]?|/&.,;¢°′″‰℃、。｡､￠，．：；？！％・･ゝゞヽヾーァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ々〻ｧｨｩｪｫｬｭｮｯｰ”〉》」』】〕）］｝｣";
const BREAK_BEFORE: &str = "([{‘“〈《「『【〔（［｛｢£¥＄￡￥+＋";

#[derive(Clone, Copy, PartialEq)]
enum Class {
    None,
    After,
    Before,
    /// Wide characters (CJK), which break anywhere.
    Wide,
}

fn class(ch: char) -> Class {
    if BREAK_AFTER.contains(ch) {
        Class::After
    } else if BREAK_BEFORE.contains(ch) {
        Class::Before
    } else if ch.width() == Some(2) {
        Class::Wide
    } else {
        Class::None
    }
}

/// Whether a wrapped line may break between `prev` and `ch`: never before whitespace, which
/// hangs off the end of the row instead, else as VS Code decides.
fn can_break(prev: Class, ch: char, cls: Class) -> bool {
    !ch.is_whitespace()
        && ((prev == Class::After && cls != Class::After)
            || (prev != Class::Before && cls == Class::Before)
            || (prev == Class::Wide && cls != Class::After)
            || (cls == Class::Wide && prev != Class::Before))
}

/// A line laid out in rows of a given width.
#[derive(Debug, PartialEq)]
pub struct Wrap {
    /// The byte offset each row starts at; the first is 0.
    pub starts: Vec<usize>,
    /// Columns rows after the first are indented by: the line's own indentation.
    pub indent: usize,
}

impl Wrap {
    pub fn rows(&self) -> usize {
        self.starts.len()
    }

    /// The row byte offset `col` is drawn on.
    pub fn row_of(&self, col: usize) -> usize {
        self.starts.partition_point(|&s| s <= col) - 1
    }

    /// Where row `r` starts and ends in `line`.
    pub fn span(&self, line: &str, r: usize) -> (usize, usize) {
        (
            self.starts[r],
            self.starts.get(r + 1).copied().unwrap_or(line.len()),
        )
    }

    /// Screen column of byte offset `col`, counted from the text's left edge.
    pub fn x_of(&self, line: &str, col: usize) -> usize {
        let r = self.row_of(col);
        let indent = if r > 0 { self.indent } else { 0 };
        indent + display_col(line, col) - display_col(line, self.starts[r])
    }

    /// Byte offset at screen column `x` on row `r`. Rows but the last stop before their final
    /// character, since their end is where the next row starts.
    pub fn col_at(&self, line: &str, r: usize, x: usize) -> usize {
        let (start, end) = self.span(line, r);
        let indent = if r > 0 { self.indent } else { 0 };
        let target = display_col(line, start).saturating_add(x.saturating_sub(indent));
        let col = col_at(line, target).clamp(start, end);
        if r + 1 < self.rows() && col == end {
            line[..end]
                .char_indices()
                .next_back()
                .map_or(start, |(i, _)| i)
        } else {
            col
        }
    }
}

/// Lays out `line` in rows `width` columns wide, breaking between words where it can and
/// indenting the rows after the first as the line is, like VS Code. 0 doesn't wrap.
pub fn wrap(line: &str, width: usize) -> Wrap {
    let single = Wrap {
        starts: vec![0],
        indent: 0,
    };
    if width == 0 || display_col(line, line.len()) <= width {
        return single;
    }
    // Each character's byte offset, column, width and class.
    let mut chars = Vec::with_capacity(line.len());
    let mut col = 0;
    for (i, ch) in line.char_indices() {
        let w = char_width(ch, col);
        chars.push((i, col, w, ch, class(ch)));
        col += w;
    }
    let lead = chars
        .iter()
        .find(|c| !c.3.is_whitespace())
        .map_or(0, |c| c.1);
    // Deep indentation would leave too little room, so it's dropped.
    let indent = if lead <= width / 2 { lead } else { 0 };
    let mut starts = vec![0];
    let (mut row, mut row_col, mut room) = (0, 0, width);
    let mut brk = None;
    let mut i = 0;
    while i < chars.len() {
        let (_, col, w, ch, cls) = chars[i];
        if i > row && can_break(chars[i - 1].4, ch, cls) {
            brk = Some(i);
        }
        if i > row && !ch.is_whitespace() && col + w - row_col > room {
            // Back to the last break, or mid-word if the row has none.
            i = brk.take().unwrap_or(i);
            starts.push(chars[i].0);
            row = i;
            row_col = chars[i].1;
            room = width - indent;
            continue;
        }
        i += 1;
    }
    Wrap { starts, indent }
}

/// Moves `delta` rows from row `sub` of line `row`, through `lines` wrapped at `width`,
/// stopping at either end.
pub fn step_rows(
    lines: &[String],
    width: usize,
    (mut row, mut sub): (usize, usize),
    delta: isize,
) -> (usize, usize) {
    let mut left = delta.unsigned_abs();
    let mut rows = wrap(&lines[row], width).rows();
    sub = sub.min(rows - 1);
    while left > 0 {
        if delta > 0 {
            let n = left.min(rows - 1 - sub);
            sub += n;
            left -= n;
            if left == 0 || row + 1 >= lines.len() {
                break;
            }
            row += 1;
            rows = wrap(&lines[row], width).rows();
            sub = 0;
        } else {
            let n = left.min(sub);
            sub -= n;
            left -= n;
            if left == 0 || row == 0 {
                break;
            }
            row -= 1;
            rows = wrap(&lines[row], width).rows();
            sub = rows - 1;
        }
        left -= 1;
    }
    (row, sub)
}

/// One level of indentation for `lines`: a tab if they indent with tabs, else two or four
/// spaces, whichever more lines use.
fn detect_indent(lines: &[String]) -> String {
    let (mut tabs, mut two, mut four) = (0, 0, 0);
    for l in lines.iter().take(2000) {
        if l.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = l.len() - l.trim_start_matches(' ').len();
        if n == l.len() {
            continue;
        }
        match n {
            2 => two += 1,
            4 => four += 1,
            _ => {}
        }
    }
    if tabs > two + four {
        "\t".into()
    } else if two > four {
        "  ".into()
    } else {
        "    ".into()
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The text of an open file: lines, a cursor and selection, and undo/redo.
pub struct Buffer {
    pub lines: Vec<String>,
    pub cur: Pos,
    /// The other end of the selection, when there is one.
    pub anchor: Option<Pos>,
    /// Display column kept while moving up and down through shorter lines.
    goal: Option<usize>,
    crlf: bool,
    /// One level of indentation: a tab or some spaces.
    indent: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Option<Edit>,
    /// Each edit gets a new version; the buffer is dirty unless it's the saved one.
    version: u64,
    next_version: u64,
    saved: u64,
    /// The first line changed since `take_changed`.
    changed: Option<usize>,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let lines: Vec<String> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        Self {
            indent: detect_indent(&lines),
            lines,
            cur: Pos::default(),
            anchor: None,
            goal: None,
            crlf,
            undo: vec![],
            redo: vec![],
            last: None,
            version: 0,
            next_version: 0,
            saved: 0,
            changed: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join(if self.crlf { "\r\n" } else { "\n" })
    }

    pub fn dirty(&self) -> bool {
        self.version != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.version;
        self.last = None;
    }

    /// The first line changed since the last call, for re-highlighting.
    pub fn take_changed(&mut self) -> Option<usize> {
        self.changed.take()
    }

    // ---------- positions ----------

    fn clamp(&self, p: Pos) -> Pos {
        let row = p.row.min(self.lines.len() - 1);
        let line = &self.lines[row];
        let mut col = p.col.min(line.len());
        while !line.is_char_boundary(col) {
            col -= 1;
        }
        Pos { row, col }
    }

    fn prev_pos(&self, p: Pos) -> Pos {
        if p.col > 0 {
            let line = &self.lines[p.row];
            let col = line[..p.col]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            Pos::new(p.row, col)
        } else if p.row > 0 {
            Pos::new(p.row - 1, self.lines[p.row - 1].len())
        } else {
            p
        }
    }

    fn next_pos(&self, p: Pos) -> Pos {
        let line = &self.lines[p.row];
        if let Some(ch) = line[p.col..].chars().next() {
            Pos::new(p.row, p.col + ch.len_utf8())
        } else if p.row + 1 < self.lines.len() {
            Pos::new(p.row + 1, 0)
        } else {
            p
        }
    }

    fn char_before(&self, p: Pos) -> Option<char> {
        self.lines[p.row][..p.col].chars().next_back()
    }

    fn char_after(&self, p: Pos) -> Option<char> {
        self.lines[p.row][p.col..].chars().next()
    }

    /// The selection's start and end, if anything is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor.filter(|&a| a != self.cur)?;
        Some(if a < self.cur {
            (a, self.cur)
        } else {
            (self.cur, a)
        })
    }

    fn text_between(&self, a: Pos, b: Pos) -> String {
        if a.row == b.row {
            return self.lines[a.row][a.col..b.col].to_string();
        }
        let mut s = self.lines[a.row][a.col..].to_string();
        for l in &self.lines[a.row + 1..b.row] {
            s.push('\n');
            s.push_str(l);
        }
        s.push('\n');
        s.push_str(&self.lines[b.row][..b.col]);
        s
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.text_between(a, b))
    }

    // ---------- movement ----------

    /// Moves the cursor, extending the selection if `select`, else dropping it.
    pub fn move_to(&mut self, p: Pos, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cur);
        } else {
            self.anchor = None;
        }
        self.cur = self.clamp(p);
        self.goal = None;
        self.last = None;
    }

    pub fn left(&mut self, select: bool) {
        match self.selection() {
            Some((a, _)) if !select => self.move_to(a, false),
            _ => self.move_to(self.prev_pos(self.cur), select),
        }
    }

    pub fn right(&mut self, select: bool) {
        match self.selection() {
            Some((_, b)) if !select => self.move_to(b, false),
            _ => self.move_to(self.next_pos(self.cur), select),
        }
    }

    /// Moves `delta` rows, keeping the screen column it started from. Rows are lines, or the
    /// rows they wrap into at `width` when it isn't 0.
    pub fn vertical(&mut self, delta: isize, select: bool, width: usize) {
        let line = &self.lines[self.cur.row];
        let w = wrap(line, width);
        let goal = self.goal.unwrap_or_else(|| w.x_of(line, self.cur.col));
        let from = (self.cur.row, w.row_of(self.cur.col));
        let (row, sub) = step_rows(&self.lines, width, from, delta);
        let line = &self.lines[row];
        let col = wrap(line, width).col_at(line, sub, goal);
        self.move_to(Pos::new(row, col), select);
        self.goal = Some(goal);
    }

    /// To the start of the wrapped row, then the first non-blank character, then the line's
    /// start.
    pub fn home(&mut self, select: bool, width: usize) {
        let line = &self.lines[self.cur.row];
        let w = wrap(line, width);
        let start = w.starts[w.row_of(self.cur.col)];
        let first = line.len() - line.trim_start().len();
        let col = if start > 0 && self.cur.col != start {
            start
        } else if self.cur.col == first {
            0
        } else {
            first
        };
        self.move_to(Pos::new(self.cur.row, col), select);
    }

    /// To the end of the wrapped row, then the line's end.
    pub fn end(&mut self, select: bool, width: usize) {
        let line = &self.lines[self.cur.row];
        let w = wrap(line, width);
        let r = w.row_of(self.cur.col);
        let end = w.col_at(line, r, usize::MAX);
        let col = if r + 1 < w.rows() && self.cur.col != end {
            end
        } else {
            line.len()
        };
        self.move_to(Pos::new(self.cur.row, col), select);
    }

    pub fn doc_start(&mut self, select: bool) {
        self.move_to(Pos::default(), select);
    }

    pub fn doc_end(&mut self, select: bool) {
        let row = self.lines.len() - 1;
        self.move_to(Pos::new(row, self.lines[row].len()), select);
    }

    pub fn select_all(&mut self) {
        self.doc_end(false);
        self.anchor = Some(Pos::default());
    }

    pub fn word_left(&mut self, select: bool) {
        let mut p = self.prev_pos(self.cur);
        while p > Pos::default() && self.char_after(p).is_none_or(|c| !is_word(c)) {
            p = self.prev_pos(p);
        }
        while self.char_before(p).is_some_and(is_word) {
            p = self.prev_pos(p);
        }
        self.move_to(p, select);
    }

    pub fn word_right(&mut self, select: bool) {
        let mut p = self.next_pos(self.cur);
        while self.char_before(p).is_none_or(|c| !is_word(c)) && self.next_pos(p) != p {
            p = self.next_pos(p);
        }
        while self.char_after(p).is_some_and(is_word) {
            p = self.next_pos(p);
        }
        self.move_to(p, select);
    }

    // ---------- edits ----------

    /// Records an undo step unless this edit continues a run of the same kind.
    fn begin(&mut self, kind: Edit) {
        if kind == Edit::Other || self.last != Some(kind) {
            self.undo.push(Snapshot {
                lines: self.lines.clone(),
                cur: self.cur,
                version: self.version,
            });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last = Some(kind);
        self.next_version += 1;
        self.version = self.next_version;
        self.goal = None;
    }

    fn touch(&mut self, row: usize) {
        self.changed = Some(self.changed.map_or(row, |c| c.min(row)));
    }

    /// Inserts `s` at `p` and returns where it ends.
    fn insert_raw(&mut self, p: Pos, s: &str) -> Pos {
        self.touch(p.row);
        let tail = self.lines[p.row].split_off(p.col);
        let mut parts = s.split('\n');
        self.lines[p.row].push_str(parts.next().unwrap_or(""));
        let mut rest: Vec<String> = parts.map(String::from).collect();
        match rest.last_mut() {
            None => {
                let line = &mut self.lines[p.row];
                let col = line.len();
                line.push_str(&tail);
                Pos::new(p.row, col)
            }
            Some(last) => {
                let col = last.len();
                last.push_str(&tail);
                let n = rest.len();
                self.lines.splice(p.row + 1..p.row + 1, rest);
                Pos::new(p.row + n, col)
            }
        }
    }

    fn remove_raw(&mut self, a: Pos, b: Pos) {
        self.touch(a.row);
        if a.row == b.row {
            self.lines[a.row].replace_range(a.col..b.col, "");
        } else {
            let tail = self.lines[b.row][b.col..].to_string();
            self.lines[a.row].truncate(a.col);
            self.lines[a.row].push_str(&tail);
            self.lines.drain(a.row + 1..=b.row);
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.remove_raw(a, b);
        self.cur = a;
        self.anchor = None;
        true
    }

    /// Types or pastes `s`, replacing the selection.
    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let typing = s.chars().count() == 1 && s != "\n" && self.selection().is_none();
        self.begin(if typing { Edit::Type } else { Edit::Other });
        self.delete_selection();
        self.cur = self.insert_raw(self.cur, &s);
    }

    /// Splits the line, carrying its indentation onto the new one.
    pub fn newline(&mut self) {
        self.begin(Edit::Other);
        self.delete_selection();
        let line = &self.lines[self.cur.row];
        let indent: String = line[..self.cur.col]
            .chars()
            .take_while(|&c| c == ' ' || c == '\t')
            .collect();
        self.cur = self.insert_raw(self.cur, &format!("\n{indent}"));
    }

    pub fn backspace(&mut self) {
        if self.selection().is_some() {
            self.begin(Edit::Other);
            self.delete_selection();
            return;
        }
        let start = self.prev_pos(self.cur);
        if start == self.cur {
            return;
        }
        self.begin(Edit::Delete);
        self.anchor = None;
        self.remove_raw(start, self.cur);
        self.cur = start;
    }

    pub fn delete(&mut self) {
        if self.selection().is_some() {
            self.begin(Edit::Other);
            self.delete_selection();
            return;
        }
        let end = self.next_pos(self.cur);
        if end == self.cur {
            return;
        }
        self.begin(Edit::Delete);
        self.anchor = None;
        self.remove_raw(self.cur, end);
    }

    /// Deletes back to the start of the word before the cursor.
    pub fn delete_word_back(&mut self) {
        if self.selection().is_none() {
            let cur = self.cur;
            self.word_left(false);
            self.anchor = Some(cur);
        }
        self.backspace();
    }

    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        self.begin(Edit::Other);
        self.delete_selection();
        Some(text)
    }

    /// Rows a line-wise edit applies to: the selected lines, or the cursor's.
    fn selected_rows(&self) -> std::ops::RangeInclusive<usize> {
        match self.selection() {
            Some((a, b)) if b.row > a.row && b.col == 0 => a.row..=b.row - 1,
            Some((a, b)) => a.row..=b.row,
            None => self.cur.row..=self.cur.row,
        }
    }

    /// Shifts the cursor and anchor on `row` by `delta` bytes, as an edit at the line start does.
    fn shift(&mut self, row: usize, delta: isize) {
        let fix = |p: &mut Pos| {
            if p.row == row {
                p.col = (p.col as isize + delta).max(0) as usize;
            }
        };
        fix(&mut self.cur);
        if let Some(a) = &mut self.anchor {
            fix(a);
        }
    }

    /// Tab: indents the selected lines, or inserts indentation at the cursor.
    pub fn indent(&mut self) {
        let multi = self.selection().is_some_and(|(a, b)| a.row != b.row);
        if !multi {
            let unit = if self.indent == "\t" {
                "\t".to_string()
            } else {
                let w = self.indent.len();
                let col = display_col(&self.lines[self.cur.row], self.cur.col);
                " ".repeat(w - col % w)
            };
            return self.insert(&unit);
        }
        self.begin(Edit::Other);
        let unit = self.indent.clone();
        for row in self.selected_rows() {
            if self.lines[row].is_empty() {
                continue;
            }
            self.touch(row);
            self.lines[row].insert_str(0, &unit);
            self.shift(row, unit.len() as isize);
        }
    }

    /// Shift+tab: removes one level of indentation from the selected lines.
    pub fn dedent(&mut self) {
        let rows = self.selected_rows();
        let unit = self.indent.len();
        let strip = |l: &str| {
            if l.starts_with('\t') {
                1
            } else {
                (l.len() - l.trim_start_matches(' ').len()).min(unit)
            }
        };
        if rows.clone().all(|r| strip(&self.lines[r]) == 0) {
            return;
        }
        self.begin(Edit::Other);
        for row in rows {
            let n = strip(&self.lines[row]);
            if n > 0 {
                self.touch(row);
                self.lines[row].replace_range(..n, "");
                self.shift(row, -(n as isize));
            }
        }
        self.cur = self.clamp(self.cur);
        self.anchor = self.anchor.map(|a| self.clamp(a));
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else {
            return false;
        };
        let cur = self.cur;
        let version = self.version;
        let lines = std::mem::replace(&mut self.lines, s.lines);
        self.redo.push(Snapshot {
            lines,
            cur,
            version,
        });
        self.restore(s.cur, s.version);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else {
            return false;
        };
        let cur = self.cur;
        let version = self.version;
        let lines = std::mem::replace(&mut self.lines, s.lines);
        self.undo.push(Snapshot {
            lines,
            cur,
            version,
        });
        self.restore(s.cur, s.version);
        true
    }

    fn restore(&mut self, cur: Pos, version: u64) {
        self.cur = self.clamp(cur);
        self.version = version;
        self.anchor = None;
        self.goal = None;
        self.last = None;
        self.changed = Some(0);
    }

    // ---------- search ----------

    /// The next match of `query` from `from` (or the previous one), wrapping around. Case is
    /// ignored unless the query has capitals.
    pub fn find(&self, query: &str, from: Pos, forward: bool) -> Option<(Pos, Pos)> {
        if query.is_empty() {
            return None;
        }
        let fold = !query.chars().any(char::is_uppercase);
        let q = if fold {
            query.to_ascii_lowercase()
        } else {
            query.to_string()
        };
        let matches = |row: usize| -> Vec<usize> {
            let line = &self.lines[row];
            let hay = if fold {
                line.to_ascii_lowercase()
            } else {
                line.clone()
            };
            hay.match_indices(&q).map(|(i, _)| i).collect()
        };
        let n = self.lines.len();
        for i in 0..=n {
            let (row, pick) = if forward {
                let row = (from.row + i) % n;
                let mut cols = matches(row).into_iter();
                let col = match i {
                    0 => cols.find(|&c| c >= from.col),
                    _ if i == n => cols.find(|&c| c < from.col),
                    _ => cols.next(),
                };
                (row, col)
            } else {
                let row = (from.row + n - i % n) % n;
                let mut cols = matches(row).into_iter().rev();
                let col = match i {
                    0 => cols.find(|&c| c < from.col),
                    _ if i == n => cols.find(|&c| c >= from.col),
                    _ => cols.next(),
                };
                (row, col)
            };
            if let Some(col) = pick {
                return Some((Pos::new(row, col), Pos::new(row, col + q.len())));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, row: usize, col: usize) -> Buffer {
        let mut b = Buffer::new(text);
        b.cur = Pos::new(row, col);
        b
    }

    #[test]
    fn round_trips_line_endings() {
        for text in ["a\nb\n", "a\r\nb", "", "\n\n"] {
            assert_eq!(Buffer::new(text).text(), text);
        }
    }

    #[test]
    fn inserts_and_deletes_across_lines() {
        let mut b = at("hello world", 0, 5);
        b.insert(",\nthere");
        assert_eq!(b.text(), "hello,\nthere world");
        assert_eq!(b.cur, Pos::new(1, 5));
        b.move_to(Pos::new(0, 6), false);
        b.delete();
        assert_eq!(b.text(), "hello,there world");
        b.backspace();
        assert_eq!(b.text(), "hellothere world");
    }

    #[test]
    fn newline_keeps_indentation() {
        let mut b = at("    let x = 1;", 0, 14);
        b.newline();
        assert_eq!(b.text(), "    let x = 1;\n    ");
        assert_eq!(b.cur, Pos::new(1, 4));
    }

    #[test]
    fn typing_undoes_as_one_step_and_tracks_dirty() {
        let mut b = at("", 0, 0);
        for c in "abc".chars() {
            b.insert(&c.to_string());
        }
        assert!(b.dirty());
        b.undo();
        assert_eq!(b.text(), "");
        assert!(!b.dirty());
        b.redo();
        assert_eq!(b.text(), "abc");
        b.mark_saved();
        b.undo();
        assert!(b.dirty());
    }

    #[test]
    fn selection_replaces_and_cuts() {
        let mut b = at("one two three", 0, 4);
        b.move_to(Pos::new(0, 7), true);
        assert_eq!(b.selected_text().as_deref(), Some("two"));
        b.insert("2");
        assert_eq!(b.text(), "one 2 three");
        b.select_all();
        assert_eq!(b.cut().as_deref(), Some("one 2 three"));
        assert_eq!(b.text(), "");
    }

    #[test]
    fn indents_and_dedents_selected_lines() {
        let mut b = Buffer::new("a\nb\n\nc");
        b.move_to(Pos::new(0, 0), false);
        b.move_to(Pos::new(3, 1), true);
        b.indent();
        assert_eq!(b.text(), "    a\n    b\n\n    c");
        b.dedent();
        assert_eq!(b.text(), "a\nb\n\nc");
        let mut b = at("x", 0, 1);
        b.indent();
        assert_eq!(b.text(), "x   ");
    }

    #[test]
    fn vertical_moves_keep_the_column() {
        let mut b = at("abcdef\nab\nabcdef", 0, 5);
        b.vertical(1, false, 0);
        assert_eq!(b.cur, Pos::new(1, 2));
        b.vertical(1, false, 0);
        assert_eq!(b.cur, Pos::new(2, 5));
    }

    fn rows(line: &str, width: usize) -> Vec<&str> {
        let w = wrap(line, width);
        (0..w.rows())
            .map(|r| {
                let (a, b) = w.span(line, r);
                &line[a..b]
            })
            .collect()
    }

    #[test]
    fn wraps_between_words_keeping_the_indent() {
        assert_eq!(rows("short", 10), ["short"]);
        assert_eq!(rows("short", 0), ["short"]);
        assert_eq!(rows("one two three four", 10), ["one two ", "three four"]);
        assert_eq!(
            rows("one two three four", 9),
            ["one two ", "three ", "four"]
        );
        assert_eq!(rows("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(rows("a.b.c(d)", 5), ["a.b.c", "(d)"]);
        // Whitespace hangs off the end instead of starting a row.
        assert_eq!(rows("abcde fgh", 5), ["abcde ", "fgh"]);
        // Continuation rows take the line's indent.
        let w = wrap("  aaa bbb ccc", 8);
        assert_eq!(w.indent, 2);
        assert_eq!(rows("  aaa bbb ccc", 8), ["  aaa ", "bbb ", "ccc"]);
        // Wide characters break anywhere, but not before closing punctuation.
        assert_eq!(rows("日本語です。", 6), ["日本語", "です。"]);
        assert_eq!(rows("日本語。です", 6), ["日本", "語。で", "す"]);
    }

    #[test]
    fn maps_columns_through_wrapped_rows() {
        let line = "  aaa bbb ccc";
        let w = wrap(line, 8);
        assert_eq!(w.row_of(6), 1);
        assert_eq!(w.x_of(line, 6), 2);
        assert_eq!(w.x_of(line, 8), 4);
        assert_eq!(w.col_at(line, 1, 3), 7);
        // Past a row's end lands before its last character, not on the next row.
        assert_eq!(w.col_at(line, 1, 99), 9);
        assert_eq!(w.col_at(line, 2, 99), line.len());
    }

    #[test]
    fn moves_through_wrapped_rows() {
        let mut b = at("one two three four\nxy", 0, 1);
        b.vertical(1, false, 9);
        assert_eq!(b.cur, Pos::new(0, 9));
        b.vertical(1, false, 9);
        assert_eq!(b.cur, Pos::new(0, 15));
        b.vertical(1, false, 9);
        assert_eq!(b.cur, Pos::new(1, 1));
        b.vertical(-2, false, 9);
        assert_eq!(b.cur, Pos::new(0, 9));
        b.end(false, 9);
        assert_eq!(b.cur, Pos::new(0, 13));
        b.end(false, 9);
        assert_eq!(b.cur, Pos::new(0, 18));
        b.home(false, 9);
        assert_eq!(b.cur, Pos::new(0, 14));
        b.home(false, 9);
        assert_eq!(b.cur, Pos::new(0, 0));
        let lines = b.lines.clone();
        assert_eq!(step_rows(&lines, 9, (0, 0), 2), (0, 2));
        assert_eq!(step_rows(&lines, 9, (0, 0), 9), (1, 0));
        assert_eq!(step_rows(&lines, 9, (1, 0), -2), (0, 1));
        assert_eq!(step_rows(&lines, 0, (1, 0), -5), (0, 0));
    }

    #[test]
    fn moves_by_word() {
        let mut b = at("foo_bar  baz(qux)", 0, 0);
        b.word_right(false);
        assert_eq!(b.cur.col, 7);
        b.word_right(false);
        assert_eq!(b.cur.col, 12);
        b.word_left(false);
        assert_eq!(b.cur.col, 9);
        b.delete_word_back();
        assert_eq!(b.text(), "baz(qux)");
    }

    #[test]
    fn finds_with_smart_case_and_wraps() {
        let b = Buffer::new("Foo bar\nfoo\nbaz Foo");
        assert_eq!(
            b.find("foo", Pos::new(0, 1), true),
            Some((Pos::new(1, 0), Pos::new(1, 3)))
        );
        assert_eq!(
            b.find("Foo", Pos::new(0, 1), true),
            Some((Pos::new(2, 4), Pos::new(2, 7)))
        );
        assert_eq!(
            b.find("Foo", Pos::new(2, 5), true),
            Some((Pos::new(0, 0), Pos::new(0, 3)))
        );
        assert_eq!(
            b.find("foo", Pos::new(1, 0), false),
            Some((Pos::new(0, 0), Pos::new(0, 3)))
        );
        assert_eq!(b.find("nope", Pos::default(), true), None);
    }

    #[test]
    fn display_columns_expand_tabs() {
        assert_eq!(display_col("\tx", 1), 4);
        assert_eq!(display_col("ab\tx", 3), 4);
        assert_eq!(col_at("\tx", 2), 0);
        assert_eq!(col_at("\tx", 4), 1);
        assert_eq!(col_at("ab", 9), 2);
    }
}
