//! Parsing Laravel log text into **Entries**. Pure — no I/O.
//!
//! Filled by Phase 3; see docs/BUILD-SPEC.md section 4.
//!
//! An **Entry** is multi-line: a header line, then the JSON context and stack
//! frames beneath it. A line matching [`HEADER_PATTERN`] closes the pending
//! Entry and opens a new one; every other line appends to the pending Entry's
//! `context` and `raw`.
//!
//! Identity is passed in, never discovered here: the caller supplies the
//! **Project** id, the file name, and the byte offset of the text being fed.
//! That is what keeps this module free of I/O and its tests meaningful.

use std::sync::OnceLock;

use regex::Regex;

use crate::model::{EntryId, Level, LogEntry, ProjectId};

/// The three header shapes that occur in practice — bare, with microseconds,
/// and with a timezone offset (BUILD-SPEC section 4). Line-anchored, so
/// header-shaped text inside a message body does not split an Entry.
const HEADER_PATTERN: &str = concat!(
    r"^\[(?P<ts>\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[+-]\d{2}:?\d{2}|Z)?)\]\s",
    r"(?P<env>\S+)\.(?P<level>[A-Z]{4,9}):\s?(?P<msg>.*)$",
);

/// Compiled once: this runs against every line of a live log tail.
fn header() -> &'static Regex {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    HEADER.get_or_init(|| Regex::new(HEADER_PATTERN).expect("HEADER_PATTERN is a valid regex"))
}

/// **Level** assigned to content that arrives before any header — the tail of
/// an Entry whose header sits before the window we were given.
///
/// `Unknown` rather than `Info`: an orphaned fragment carries no severity of its
/// own. `Unknown` ranks below `Debug`, so it cannot inflate the **Activity**
/// badge (D8), while staying distinguishable from something the application
/// really did log at INFO.
pub const HEADERLESS_LEVEL: Level = Level::Unknown;

/// **Level** assigned when the header's level text is not one of the eight PSR
/// levels — a custom Monolog level still matches the header shape.
///
/// Same reasoning as [`HEADERLESS_LEVEL`]: never panic, never inflate, and never
/// impersonate a severity the writer did not use.
pub const UNKNOWN_LEVEL: Level = Level::Unknown;

/// The eight PSR-3 levels Laravel writes. Anything else is [`UNKNOWN_LEVEL`].
fn level_from(text: &str) -> Level {
    match text {
        "DEBUG" => Level::Debug,
        "INFO" => Level::Info,
        "NOTICE" => Level::Notice,
        "WARNING" => Level::Warning,
        "ERROR" => Level::Error,
        "CRITICAL" => Level::Critical,
        "ALERT" => Level::Alert,
        "EMERGENCY" => Level::Emergency,
        _ => UNKNOWN_LEVEL,
    }
}

/// Drop the `\r` of a `\r\n` terminator. Byte-level, before decoding, so it
/// cannot be confused with a `\r` inside a multi-byte sequence — `\r` is ASCII
/// and never appears as a UTF-8 continuation byte.
fn strip_carriage_return(line: &[u8]) -> &[u8] {
    match line.split_last() {
        Some((b'\r', head)) => head,
        _ => line,
    }
}

/// An Entry still open: more lines may yet belong to it.
struct Pending {
    entry: LogEntry,
    /// Whether any context line has been appended. A separate flag rather than
    /// `context.is_empty()`, because the first context line may legitimately be
    /// blank and must still be retained.
    has_context: bool,
}

/// Accumulates lines into **Entries** across as many feeds as the caller makes.
///
/// The watcher reads a file in chunks and an Entry can span two reads, so the
/// pending Entry is held across calls and revealed by [`Accumulator::pending`].
/// Phase 6 emits that trailing Entry immediately and revises it in place under
/// its stable id (D2).
pub struct Accumulator {
    project_id: ProjectId,
    file: String,
    pending: Option<Pending>,
}

impl Accumulator {
    pub fn new(project_id: ProjectId, file: &str) -> Self {
        Self {
            project_id,
            file: file.to_owned(),
            pending: None,
        }
    }

    /// Feed a chunk of raw bytes that begins at `offset` in the file.
    ///
    /// Decoding is lossy and per line, so invalid UTF-8 can never panic and
    /// can never shift the byte offsets the ids are built from. Returns the
    /// Entries this chunk *closed*; the last one stays pending.
    ///
    /// The chunk is expected to end on a line boundary — the watcher processes
    /// only through the last `\n` (BUILD-SPEC section 5). A trailing partial
    /// line is treated as a whole line rather than silently dropped.
    ///
    /// A `\r\n` terminator is treated as a terminator, not as content: the `\r`
    /// is dropped before matching, so a log written on Windows yields the same
    /// `message`, `context`, and `raw` as the same log written on Unix. Content
    /// whitespace — indentation, trailing spaces — is still verbatim.
    pub fn push_bytes(&mut self, offset: u64, bytes: &[u8]) -> Vec<LogEntry> {
        let mut closed = Vec::new();
        let mut pos = 0usize;

        while pos < bytes.len() {
            let rest = &bytes[pos..];
            let (line, consumed) = match rest.iter().position(|&byte| byte == b'\n') {
                // Only a `\r` immediately before the `\n` is a terminator.
                Some(end) => (strip_carriage_return(&rest[..end]), end + 1),
                None => (rest, rest.len()),
            };

            let decoded = String::from_utf8_lossy(line);
            if let Some(entry) = self.push_line(offset + pos as u64, &decoded) {
                closed.push(entry);
            }

            pos += consumed;
        }

        closed
    }

    /// [`Accumulator::push_bytes`] for text that is already valid UTF-8.
    pub fn push_str(&mut self, offset: u64, text: &str) -> Vec<LogEntry> {
        self.push_bytes(offset, text.as_bytes())
    }

    /// The Entry still open, if any. Emitted immediately and revised in place
    /// as later lines arrive under the same id (D2).
    pub fn pending(&self) -> Option<&LogEntry> {
        self.pending.as_ref().map(|pending| &pending.entry)
    }

    /// Close the pending Entry — at end of input, or before a **Break**.
    /// Idempotent.
    pub fn flush(&mut self) -> Option<LogEntry> {
        self.pending.take().map(|pending| pending.entry)
    }

    /// Open a new pending Entry. The identity fields — id, Project, file, and
    /// offset — are derived here and nowhere else, so the header-initiated and
    /// headerless paths cannot drift apart (D2 depends on one id scheme).
    ///
    /// `raw` is the line verbatim, never reassembled from the parts above:
    /// whitespace a developer pastes into a bug report must survive
    /// (BUILD-SPEC section 2).
    fn open(
        &mut self,
        offset: u64,
        timestamp: &str,
        env: &str,
        level: Level,
        message: &str,
        line: &str,
    ) {
        self.pending = Some(Pending {
            entry: LogEntry {
                id: EntryId::new(&self.file, offset),
                project_id: self.project_id.clone(),
                file: self.file.clone(),
                offset,
                timestamp: timestamp.to_owned(),
                env: env.to_owned(),
                level,
                message: message.to_owned(),
                context: String::new(),
                raw: line.to_owned(),
            },
            has_context: false,
        });
    }

    /// One line, at its byte offset in the file. Returns the Entry it closed.
    fn push_line(&mut self, offset: u64, line: &str) -> Option<LogEntry> {
        match header().captures(line) {
            Some(caps) => {
                let closed = self.flush();
                let level = level_from(&caps["level"]);
                self.open(offset, &caps["ts"], &caps["env"], level, &caps["msg"], line);
                closed
            }
            None => {
                match &mut self.pending {
                    Some(pending) => {
                        if pending.has_context {
                            pending.entry.context.push('\n');
                        }
                        pending.entry.context.push_str(line);
                        pending.has_context = true;
                        pending.entry.raw.push('\n');
                        pending.entry.raw.push_str(line);
                    }
                    // Content before any header: the tail of an Entry whose
                    // header is outside our window. Kept rather than discarded
                    // — its frames are still what the user wants to copy.
                    None => self.open(offset, "", "", HEADERLESS_LEVEL, line, line),
                }
                None
            }
        }
    }
}

/// Parse a self-contained slice of log text into **Entries**, closing the last
/// one. For incremental feeding use [`Accumulator`].
pub fn parse_entries(project_id: &ProjectId, file: &str, offset: u64, text: &str) -> Vec<LogEntry> {
    parse_bytes(project_id, file, offset, text.as_bytes())
}

/// [`parse_entries`] over raw bytes; invalid UTF-8 decodes lossily.
pub fn parse_bytes(project_id: &ProjectId, file: &str, offset: u64, bytes: &[u8]) -> Vec<LogEntry> {
    let mut acc = Accumulator::new(project_id.clone(), file);
    let mut entries = acc.push_bytes(offset, bytes);
    entries.extend(acc.flush());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EntryId, Level, ProjectId};
    use std::path::Path;

    fn project() -> ProjectId {
        ProjectId::from_canonical(Path::new("/Users/dev/projects/api"))
    }

    const FILE: &str = "laravel-2026-08-14.log";

    fn parse(text: &str) -> Vec<crate::model::LogEntry> {
        parse_entries(&project(), FILE, 0, text)
    }

    // ---- Case 1 -----------------------------------------------------------

    #[test]
    fn content_without_a_severity_is_unknown_and_never_info() {
        // Pins the decision, not just the constant: a PHP fatal or stderr line
        // with no Monolog header must not impersonate something the app logged
        // at INFO, and must rank below Debug so it cannot inflate Activity.
        assert_eq!(HEADERLESS_LEVEL, Level::Unknown);
        assert_eq!(UNKNOWN_LEVEL, Level::Unknown);
        assert_ne!(HEADERLESS_LEVEL, Level::Info);
        assert!(Level::Unknown < Level::Debug);
    }

    #[test]
    fn header_splits_into_timestamp_env_level_and_message() {
        let entries = parse("[2026-08-14 01:28:00] local.ERROR: msg\n");

        assert_eq!(entries.len(), 1, "{entries:#?}");
        let entry = &entries[0];
        assert_eq!(entry.timestamp, "2026-08-14 01:28:00");
        assert_eq!(entry.env, "local");
        assert_eq!(entry.level, Level::Error);
        assert_eq!(entry.message, "msg");
        assert_eq!(entry.context, "");
        assert_eq!(entry.raw, "[2026-08-14 01:28:00] local.ERROR: msg");
        assert_eq!(entry.file, FILE);
        assert_eq!(entry.offset, 0);
        assert_eq!(entry.id, EntryId::new(FILE, 0));
        assert_eq!(entry.project_id, project());
    }

    // ---- Case 2 -----------------------------------------------------------

    #[test]
    fn header_with_microseconds_parses() {
        let entries = parse("[2026-08-14 01:28:00.123456] production.WARNING: slow query\n");

        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].timestamp, "2026-08-14 01:28:00.123456");
        assert_eq!(entries[0].env, "production");
        assert_eq!(entries[0].level, Level::Warning);
        assert_eq!(entries[0].message, "slow query");
    }

    // ---- Case 3 -----------------------------------------------------------

    #[test]
    fn header_with_timezone_offset_parses() {
        let entries = parse(concat!(
            "[2026-08-14 01:28:00+00:00] local.INFO: zero offset\n",
            "[2026-08-14T01:28:00.500-05:30] local.DEBUG: tee and half hour\n",
            "[2026-08-14 01:28:00Z] local.NOTICE: zulu\n",
        ));

        assert_eq!(entries.len(), 3, "{entries:#?}");
        assert_eq!(entries[0].timestamp, "2026-08-14 01:28:00+00:00");
        assert_eq!(entries[0].level, Level::Info);
        assert_eq!(entries[1].timestamp, "2026-08-14T01:28:00.500-05:30");
        assert_eq!(entries[1].level, Level::Debug);
        assert_eq!(entries[2].timestamp, "2026-08-14 01:28:00Z");
        assert_eq!(entries[2].level, Level::Notice);
    }

    // ---- Case 4 -----------------------------------------------------------

    fn trace_of(frames: usize) -> String {
        (0..frames)
            .map(|n| {
                format!(
                    "#{n} /var/www/app/Http/Controllers/UserController.php({n}): App\\Foo->bar()"
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn forty_seven_line_trace_forms_one_entry_with_verbatim_raw() {
        let trace = trace_of(47);
        let text =
            format!("[2026-08-14 01:28:00] local.ERROR: Undefined variable $user\n{trace}\n");

        let entries = parse(&text);

        assert_eq!(entries.len(), 1, "a trace must not split the Entry");
        let entry = &entries[0];
        assert_eq!(entry.message, "Undefined variable $user");
        assert_eq!(
            entry.context.lines().count(),
            47,
            "every frame belongs to context"
        );
        assert_eq!(entry.context, trace);
        assert_eq!(
            entry.raw,
            text.trim_end_matches('\n'),
            "raw is verbatim, never reassembled"
        );
    }

    #[test]
    fn raw_preserves_whitespace_that_reassembly_would_normalise() {
        // Section 2: a developer pasting into a bug report may need this
        // whitespace, so `raw` is captured, not rebuilt from the parts.
        let text = "[2026-08-14 01:28:00] local.ERROR:   double spaced   \n\t  indented frame  \n";

        let entries = parse(text);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].raw, text.trim_end_matches('\n'));
        assert!(entries[0].raw.contains("\t  indented frame  "));
        // `message` and `context` are equally verbatim: the pattern eats one
        // optional space after the colon and nothing else. A parser that
        // trimmed here would still satisfy the `raw` assertions above.
        assert_eq!(entries[0].message, "  double spaced   ");
        assert_eq!(entries[0].context, "\t  indented frame  ");
    }

    // ---- Case 5 -----------------------------------------------------------

    #[test]
    fn json_context_line_stays_attached_to_its_header() {
        let json = r#"{"exception":"[object] (ErrorException(code: 0): Undefined at /app/x.php:42)","userId":7}"#;
        let text = format!("[2026-08-14 01:28:00] local.ERROR: Something broke\n{json}\n[2026-08-14 01:28:01] local.INFO: next\n");

        let entries = parse(&text);

        assert_eq!(entries.len(), 2, "{entries:#?}");
        assert_eq!(entries[0].context, json);
        assert_eq!(entries[1].message, "next");
        assert_eq!(entries[1].context, "");
    }

    // ---- Case 6 -----------------------------------------------------------

    #[test]
    fn stacktrace_marker_and_blank_lines_do_not_split_an_entry() {
        let text = "[2026-08-14 01:28:00] local.ERROR: boom\n\n[stacktrace]\n#0 /app/x.php(42)\n\n#1 /app/y.php(7)\n";

        let entries = parse(text);

        assert_eq!(entries.len(), 1, "{entries:#?}");
        let entry = &entries[0];
        assert_eq!(
            entry.context,
            "\n[stacktrace]\n#0 /app/x.php(42)\n\n#1 /app/y.php(7)"
        );
        assert!(entry.raw.contains("[stacktrace]"));
        assert_eq!(
            entry.raw,
            text.trim_end_matches('\n'),
            "blank lines retained"
        );
    }

    // ---- Case 7 -----------------------------------------------------------

    #[test]
    fn content_before_any_header_yields_a_headerless_entry() {
        let text = "#0 /app/orphan.php(1): tail of a previous read\n#1 /app/orphan.php(2)\n[2026-08-14 01:28:00] local.ERROR: first real header\n";

        let entries = parse(text);

        assert_eq!(entries.len(), 2, "{entries:#?}");
        let orphan = &entries[0];
        assert_eq!(orphan.timestamp, "");
        assert_eq!(orphan.env, "");
        assert_eq!(orphan.level, HEADERLESS_LEVEL);
        assert_eq!(
            orphan.message,
            "#0 /app/orphan.php(1): tail of a previous read"
        );
        assert_eq!(orphan.context, "#1 /app/orphan.php(2)");
        assert_eq!(
            orphan.raw,
            "#0 /app/orphan.php(1): tail of a previous read\n#1 /app/orphan.php(2)"
        );
        assert_eq!(orphan.offset, 0);
        assert_eq!(entries[1].message, "first real header");
    }

    // ---- Case 8 -----------------------------------------------------------

    #[test]
    fn header_shaped_text_inside_a_message_body_does_not_split() {
        let text = concat!(
            "[2026-08-14 01:28:00] local.ERROR: relaying [2026-08-14 01:27:59] local.INFO: inner\n",
            "  [2026-08-14 01:27:58] local.WARNING: indented, so not a header\n",
            "quoted \"[2026-08-14 01:27:57] local.ALERT: nope\"\n",
        );

        let entries = parse(text);

        assert_eq!(
            entries.len(),
            1,
            "the pattern is line-anchored: {entries:#?}"
        );
        assert_eq!(entries[0].level, Level::Error);
        assert_eq!(
            entries[0].message,
            "relaying [2026-08-14 01:27:59] local.INFO: inner"
        );
        assert_eq!(entries[0].context.lines().count(), 2);
    }

    // ---- Case 9 -----------------------------------------------------------

    #[test]
    fn invalid_utf8_decodes_lossily_without_panicking() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"[2026-08-14 01:28:00] local.ERROR: caf");
        bytes.extend_from_slice(&[0xFF, 0xFE, 0x9F]); // never valid UTF-8
        bytes.extend_from_slice(b" broke\n#0 /app/x.php(42)\n");
        bytes.extend_from_slice(&[0x80]); // lone continuation byte
        bytes.push(b'\n');

        let entries = parse_bytes(&project(), FILE, 0, &bytes);

        assert_eq!(entries.len(), 1, "{entries:#?}");
        // Pinned exactly: each invalid byte becomes one U+FFFD, and the split
        // into lines happens on bytes, so no byte is mis-attributed across it.
        assert_eq!(entries[0].message, "caf\u{FFFD}\u{FFFD}\u{FFFD} broke");
        assert_eq!(entries[0].context, "#0 /app/x.php(42)\n\u{FFFD}");
    }

    #[test]
    fn arbitrary_bytes_yield_a_sane_parse_and_never_panic() {
        assert!(parse_bytes(&project(), FILE, 0, b"").is_empty());

        // Blank lines with no header open *one* headerless Entry, not one per
        // line — a blank line is content of the pending Entry (Case 6).
        let blanks = parse_bytes(&project(), FILE, 0, b"\n\n\n");
        assert_eq!(blanks.len(), 1, "{blanks:#?}");
        assert_eq!(blanks[0].level, HEADERLESS_LEVEL);
        assert_eq!(blanks[0].raw, "\n\n");
        let one_blank = parse_bytes(&project(), FILE, 0, b"\n");
        assert_eq!(one_blank.len(), 1, "{one_blank:#?}");
        assert_eq!(one_blank[0].raw, "");

        // A bare `[` is not a header: it must not be split into fields.
        let bracket = parse_bytes(&project(), FILE, 0, b"[");
        assert_eq!(bracket.len(), 1, "{bracket:#?}");
        assert_eq!(bracket[0].timestamp, "");
        assert_eq!(bracket[0].env, "");
        assert_eq!(bracket[0].level, HEADERLESS_LEVEL);
        assert_eq!(bracket[0].message, "[");

        // 64 invalid bytes: one headerless Entry of replacement characters.
        let garbage = parse_bytes(&project(), FILE, 0, &[0xFF; 64]);
        assert_eq!(garbage.len(), 1, "{garbage:#?}");
        assert_eq!(garbage[0].raw, "\u{FFFD}".repeat(64));

        // A header with neither a space nor a message after the colon still
        // parses as a header, not as headerless content.
        let bare = parse_bytes(&project(), FILE, 0, b"[2026-08-14 01:28:00] local.ERROR:");
        assert_eq!(bare.len(), 1, "{bare:#?}");
        assert_eq!(bare[0].level, Level::Error);
        assert_eq!(bare[0].message, "");

        let unknown = parse_bytes(
            &project(),
            FILE,
            0,
            b"[2026-08-14 01:28:00] local.NOPE: unknown level",
        );
        assert_eq!(unknown.len(), 1, "{unknown:#?}");
        // "NOPE" is 4 uppercase letters, so it matches the header shape.
        assert_eq!(unknown[0].level, UNKNOWN_LEVEL);
        assert_eq!(unknown[0].message, "unknown level");
    }

    // ---- CRLF -------------------------------------------------------------

    #[test]
    fn crlf_terminators_do_not_leak_a_carriage_return_into_any_field() {
        // A Laravel project on a Windows host, or a log touched by a CRLF
        // editor, writes `\r\n`. The `\r` is a terminator, not content: a
        // stray one would corrupt the Copy payload (D1) and the string Search
        // matches against (D7) while remaining invisible in the UI.
        let entries = parse(concat!(
            "[2026-08-14 01:28:00] local.ERROR: boom\r\n",
            "#0 /app/x.php(42)\r\n",
            "[2026-08-14 01:28:01] local.INFO: next\r\n",
        ));

        assert_eq!(entries.len(), 2, "{entries:#?}");
        assert_eq!(entries[0].message, "boom");
        assert_eq!(entries[0].context, "#0 /app/x.php(42)");
        assert_eq!(
            entries[0].raw,
            "[2026-08-14 01:28:00] local.ERROR: boom\n#0 /app/x.php(42)"
        );
        assert_eq!(entries[1].message, "next");
        assert!(
            entries.iter().all(|entry| !entry.raw.contains('\r')
                && !entry.message.contains('\r')
                && !entry.context.contains('\r')),
            "{entries:#?}"
        );

        // Offsets still count the terminator bytes actually in the file, so
        // ids stay addressable back into it.
        let first = "[2026-08-14 01:28:00] local.ERROR: boom\r\n#0 /app/x.php(42)\r\n";
        assert_eq!(entries[1].offset, first.len() as u64);
        assert_eq!(entries[1].id, EntryId::new(FILE, first.len() as u64));
    }

    #[test]
    fn a_lone_carriage_return_inside_a_line_is_content_not_a_terminator() {
        // Only the `\r` immediately before the `\n` is stripped.
        let entries = parse("[2026-08-14 01:28:00] local.ERROR: a\rb\r\r\n");

        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].message, "a\rb\r");
    }

    // ---- Identity, offsets, and the incremental accumulator ---------------

    #[test]
    fn entry_offset_is_the_offset_of_its_header_within_the_file() {
        let first = "[2026-08-14 01:28:00] local.ERROR: one\n";
        let text = format!("{first}[2026-08-14 01:28:01] local.INFO: two\n");
        let base = 4_096;

        let entries = parse_entries(&project(), FILE, base, &text);

        assert_eq!(entries[0].offset, base);
        assert_eq!(entries[0].id, EntryId::new(FILE, base));
        assert_eq!(entries[1].offset, base + first.len() as u64);
        assert_eq!(entries[1].id, EntryId::new(FILE, base + first.len() as u64));
    }

    #[test]
    fn an_entry_split_across_two_chunks_keeps_one_id_and_grows_in_place() {
        // Phase 6 feeds the watcher's reads in chunks; an Entry can span two
        // of them and is emitted immediately, then revised in place (D2).
        let head = "[2026-08-14 01:28:00] local.ERROR: boom\n";
        let tail = "#0 /app/x.php(42)\n";

        let mut acc = Accumulator::new(project(), FILE);

        assert!(acc.push_str(0, head).is_empty(), "nothing closed yet");
        let opened = acc.pending().expect("pending Entry is revealed").clone();
        assert_eq!(opened.id, EntryId::new(FILE, 0));
        assert_eq!(opened.context, "");

        assert!(acc.push_str(head.len() as u64, tail).is_empty());
        let revised = acc.pending().expect("still pending").clone();
        assert_eq!(revised.id, opened.id, "same id — the client upserts (D2)");
        assert_eq!(revised.context, "#0 /app/x.php(42)");
        assert_eq!(revised.raw, format!("{head}{tail}").trim_end().to_string());

        let closed = acc.flush().expect("flush closes the pending Entry");
        assert_eq!(closed, revised);
        assert!(acc.pending().is_none());
        assert!(acc.flush().is_none(), "flush is idempotent");
    }

    #[test]
    fn a_headerless_entry_split_across_two_chunks_keeps_one_id() {
        // D6's opening window doubles its read while walking back to the first
        // header, so the orphan tail can arrive across more than one feed. Its
        // id must be pinned to the offset of its *first* line, exactly as a
        // header-initiated Entry's is (D2).
        let head = "#0 /app/orphan.php(1)\n";
        let tail = "#1 /app/orphan.php(2)\n";
        let base = 4_096;

        let mut acc = Accumulator::new(project(), FILE);

        assert!(acc.push_str(base, head).is_empty(), "nothing closed yet");
        let opened = acc.pending().expect("orphan is pending").clone();
        assert_eq!(opened.id, EntryId::new(FILE, base));
        assert_eq!(opened.offset, base);
        assert_eq!(opened.level, HEADERLESS_LEVEL);

        assert!(acc.push_str(base + head.len() as u64, tail).is_empty());
        let revised = acc.pending().expect("still pending").clone();
        assert_eq!(revised.id, opened.id, "id must not drift to the later line");
        assert_eq!(revised.offset, base, "offset stays that of the first line");
        assert_eq!(revised.message, "#0 /app/orphan.php(1)");
        assert_eq!(revised.context, "#1 /app/orphan.php(2)");

        assert_eq!(acc.flush(), Some(revised));
    }

    #[test]
    fn a_header_in_a_later_chunk_closes_the_pending_entry() {
        let first = "[2026-08-14 01:28:00] local.ERROR: one\n";
        let second = "[2026-08-14 01:28:01] local.INFO: two\n";

        let mut acc = Accumulator::new(project(), FILE);
        acc.push_str(0, first);
        let closed = acc.push_str(first.len() as u64, second);

        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].message, "one");
        assert_eq!(closed[0].offset, 0);
        assert_eq!(acc.pending().map(|e| e.offset), Some(first.len() as u64));
    }

    #[test]
    fn a_line_split_across_chunks_is_not_supported_by_design() {
        // The watcher processes only through the last `\n` (section 5), so a
        // chunk always ends on a line boundary. A trailing partial line is
        // treated as a whole line rather than silently dropped.
        let mut acc = Accumulator::new(project(), FILE);
        acc.push_str(0, "[2026-08-14 01:28:00] local.ERROR: no trailing newline");

        assert_eq!(
            acc.pending().map(|e| e.message.as_str()),
            Some("no trailing newline")
        );
    }

    // ---- Level mapping ----------------------------------------------------

    #[test]
    fn every_psr_level_maps_and_an_unknown_one_falls_back() {
        for (text, expected) in [
            ("DEBUG", Level::Debug),
            ("INFO", Level::Info),
            ("NOTICE", Level::Notice),
            ("WARNING", Level::Warning),
            ("ERROR", Level::Error),
            ("CRITICAL", Level::Critical),
            ("ALERT", Level::Alert),
            ("EMERGENCY", Level::Emergency),
        ] {
            let line = format!("[2026-08-14 01:28:00] local.{text}: m\n");
            assert_eq!(parse(&line)[0].level, expected, "{text}");
        }

        // A custom Monolog level still matches the header shape. It must not
        // panic, and must not inflate the Activity badge (D8).
        let entries = parse("[2026-08-14 01:28:00] local.TRACE: custom\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, UNKNOWN_LEVEL);
        assert_eq!(entries[0].env, "local");
        assert_eq!(entries[0].message, "custom");
    }

    #[test]
    fn a_lowercase_or_malformed_level_is_not_a_header() {
        // Guards the [A-Z]{4,9} class: these lines join the pending Entry
        // rather than opening a new one.
        let text = concat!(
            "[2026-08-14 01:28:00] local.ERROR: real\n",
            "[2026-08-14 01:28:01] local.error: lowercase\n",
            "[2026-08-14 01:28:02] local.ERR: too short\n",
            "[2026-08-14 01:28:03] local.EXTRAORDINARY: too long\n",
            "[2026-08-14 01:28:04] localERROR: no dot\n",
        );

        let entries = parse(text);

        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].context.lines().count(), 4);
    }

    #[test]
    fn a_header_with_an_empty_message_parses() {
        let entries = parse("[2026-08-14 01:28:00] local.ERROR:\n");

        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].message, "");
        assert_eq!(entries[0].level, Level::Error);
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse("").is_empty());
    }
}
