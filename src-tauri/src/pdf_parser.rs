use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ParsedRecord {
    pub first_name: String,
    pub last_name: String,
    pub role: String, // always "head" — this source has no gender/role markers to key off of
    /// Second head sharing this same directory entry (a couple), if any.
    /// Never more than one — confirmed against real data: an entry lists
    /// at most 2 co-heads; anyone else at the address gets their own entry.
    pub first_name_2: Option<String>,
    pub last_name_2: Option<String>,
    pub role_2: Option<String>,
    /// Phone/email captured off the directory per head — display-only,
    /// never edited through the app.
    pub phone_1: Option<String>,
    pub email_1: Option<String>,
    pub phone_2: Option<String>,
    pub email_2: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// True when the source had unparsed trailing lines for this entry —
    /// almost always the space-separated minors list, which has no
    /// delimiters and can't be safely split into individual names. The
    /// names themselves are deliberately never stored anywhere, only
    /// this flag (still surfaced as an import warning for visibility).
    pub has_minors: bool,
    /// Free-text notes only — never auto-populated by the parser.
    pub comments: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ParseWarning {
    pub context: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ParseResult {
    pub records: Vec<ParsedRecord>,
    pub warnings: Vec<ParseWarning>,
}

fn csz_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?P<city>[A-Za-z .'-]+)\s+(?P<state>[A-Z]{2})\s+(?P<zip>\d{5}(-\d{4})?-?)$").unwrap())
}
fn coord_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^-?\d+\.\d+,\s*-?\d+\.\d+$").unwrap())
}
fn phone_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\(?\+?\d[\d\s().\-+]{5,}\d\)?$").unwrap())
}
fn name_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Lastname(s), Firstname(s)[ & Firstname(s)]" — the one reliable
    // anchor in this source: every household record starts with a line
    // containing a top-level comma, and nothing else in the document does
    // (addresses, coordinates, phones, minors lists all lack a comma).
    //
    // Issue #56: the leading class used to be [A-Z] — the ASCII-only
    // class, which never matches a surname starting with Á, Ñ, Ö, Ø, Č,
    // Ł, etc. A missed match here isn't a rejected record, it's a
    // record that's never detected as a block start at all — its lines
    // get silently absorbed into the PRECEDING household's block. \p{Lu}
    // is the Unicode "uppercase letter" class (regex crate supports
    // Unicode classes by default) and covers the same ASCII letters plus
    // every accented/non-Latin uppercase letter. The {0,60} cap on the
    // rest of the surname is left as-is (open question, needs checking
    // against the real directory before touching it — see issue #56).
    RE.get_or_init(|| Regex::new(r"^([\p{Lu}][^,\n]{0,60}), (.+)$").unwrap())
}
/// Rejects a coordinate that failed to parse, or parsed to NaN/inf
/// (`f64::from_str` accepts both), or falls outside the valid range for
/// its axis. Same bounds `settings::validate_route_start` already uses
/// for the one coordinate the user types by hand — extended here to the
/// thousands that arrive by import (#24).
fn valid_coord(v: Option<f64>, min: f64, max: f64) -> Option<f64> {
    v.filter(|n| n.is_finite() && (min..=max).contains(n))
}

fn street_start_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d").unwrap())
}

/// "PO Box <number>" doesn't follow the standard <number> <words> street
/// pattern at all — street_start_re (^\d) will never match "PO", so a
/// glued line like "Jacqueline Harlow PO Box 42" would otherwise be
/// parsed as all-names. Checks whether tokens[i..] starts with "PO"
/// "Box" (case-insensitive, optional trailing period on "PO" — "P.O."),
/// e.g. tokens ["PO","Box","42"] or ["P.O.","Box","42"].
fn is_po_box_start(tokens: &[&str], i: usize) -> bool {
    let po = tokens.get(i).map(|t| t.trim_end_matches('.').eq_ignore_ascii_case("po")).unwrap_or(false);
    let bx = tokens.get(i + 1).map(|t| t.trim_end_matches('.').eq_ignore_ascii_case("box")).unwrap_or(false);
    po && bx
}

fn lone_letter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Z]$").unwrap())
}
fn footer_timestamp_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{1,2}/\d{1,2}/\d{2,4}, \d{1,2}:\d{2} ?[AP]M\b").unwrap())
}
fn ward_title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(Ward|Branch) - \d+$").unwrap())
}

/// Regexes here MUST be cached (OnceLock), not compiled inline — this
/// function runs once per line, and a real directory PDF is thousands of
/// lines. Compiling 3 fresh regexes per line was actually happening here
/// (~7,800 compiles for a 546-record file), and regex compilation is not
/// free — that was the entire multi-second import freeze, not the DB
/// insert loop this file also emits progress events for.
fn is_boilerplate_line(line: &str) -> bool {
    if line.contains("churchofjesuschrist.org") {
        return true;
    }
    if line.starts_with('©') {
        return true;
    }
    if lone_letter_re().is_match(line) {
        return true; // lone section-divider letter ("A", "B", …)
    }
    if footer_timestamp_re().is_match(line) {
        return true; // page footer timestamp
    }
    if ward_title_re().is_match(line) {
        return true; // repeated page-header title
    }
    false
}

/// Splits raw extracted text into non-empty, trimmed lines with page
/// header/footer boilerplate stripped. Deliberately pattern-based only —
/// an earlier frequency-based pass ("anything repeated 3+ times is
/// boilerplate") also stripped legitimate data: short city/state/zip
/// combinations like "Winchester VA 22602" recur across many unrelated
/// households in a real directory and aren't boilerplate at all.
fn clean_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !is_boilerplate_line(l))
        .collect()
}

fn name_matches_head(line: &str, head_first_token: &str) -> bool {
    let a = line.trim().to_lowercase();
    let b = head_first_token.trim().to_lowercase();
    a == b || a.starts_with(&format!("{b} ")) || b.starts_with(&format!("{a} "))
}

/// If `rest` (everything after "Last, ") has an address glued onto the
/// same physical line — e.g. "Jacqueline Harlow 323 Russelcroft Rd" —
/// splits it into (names, Some(address_fragment)). Otherwise (names, None).
fn split_name_and_glued_address(rest: &str) -> (String, Option<String>) {
    let tokens: Vec<&str> = rest.split(' ').collect();
    for (i, tok) in tokens.iter().enumerate() {
        if street_start_re().is_match(tok) || is_po_box_start(&tokens, i) {
            let names = tokens[..i].join(" ").trim().to_string();
            let addr = tokens[i..].join(" ").trim().to_string();
            if !names.is_empty() && !addr.is_empty() {
                return (names, Some(addr));
            }
        }
    }
    (rest.to_string(), None)
}

fn parse_heads(names_part: &str) -> Vec<String> {
    names_part.split(" & ").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Splits a head's full given-name string into ("First Middle", "First")
/// — the second element is just the bare first token, used for matching
/// name-repeat lines elsewhere in the block.
fn split_first_middle(h: &str) -> (String, String) {
    let mut parts = h.splitn(2, ' ');
    let first = parts.next().unwrap_or("").to_string();
    let middle = parts.next().unwrap_or("").to_string();
    (if middle.is_empty() { first.clone() } else { format!("{first} {middle}") }, first)
}

/// Extracts text via poppler's `pdftotext -raw`, not the `pdf-extract`
/// crate. `pdf-extract` corrupts this PDF's numbers specifically — it
/// misreads this font's per-glyph positioning as word breaks between
/// individual digits ("100" comes out "1 0 0"), while letters are
/// unaffected. Confirmed against the real file: `-raw` mode gives correct
/// digit grouping AND the right reading order (name-then-address,
/// glued onto one line exactly like this grammar already expects). Do
/// not swap this back to `pdf-extract` without re-confirming that bug
/// is actually fixed upstream first.
///
/// `pdftotext` ships bundled, not as a system dependency — end users
/// never need poppler installed separately. On Windows/macOS it's a
/// Tauri `externalBin` sidecar (tauri.conf.json's bundle.externalBin,
/// src-tauri/binaries/). On Linux it's shipped as a plain bundle
/// resource instead (src-tauri/tauri.linux.conf.json), because the
/// .deb/.rpm bundler installs sidecars straight into /usr/bin/<name>,
/// which collides with the system poppler-utils package there — see
/// the cfg(target_os = "linux") branch below for the split. Resolving
/// either path requires an `AppHandle`, so this is async.
pub async fn parse_pdf(app: &tauri::AppHandle, path: &Path) -> anyhow::Result<ParseResult> {
    #[cfg(not(target_os = "linux"))]
    use tauri_plugin_shell::ShellExt;

    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("PDF path is not valid UTF-8: {}", path.display()))?;

    // Linux: run the bundled poppler-utils binary as a plain resource file,
    // not a Tauri `externalBin` sidecar. Sidecars get installed straight
    // into /usr/bin/<name> by the .deb/.rpm bundler — colliding with the
    // system poppler-utils package, which also owns /usr/bin/pdftotext.
    // Resources land in the app's own private resource dir instead, so
    // there's nothing to collide with. The binary is a busybox-style
    // multicall executable that picks which poppler tool to act as by
    // reading its own argv[0] — it must still be invoked under the exact
    // name "pdftotext" (never renamed) or it won't recognize itself.
    // Running it by its resolved path (rather than through the sidecar
    // API) sets argv[0] to that path, whose final component is
    // "pdftotext" — the resource file is deliberately never renamed, so
    // dispatch keeps working. Blocking Command is used deliberately here:
    // this only runs once per user-initiated PDF import, not a hot path,
    // and pulling in the async tokio::process API just for this one call
    // isn't worth a new direct Cargo dependency.
    #[cfg(target_os = "linux")]
    let output = {
        use tauri::Manager;
        let resource_path = app
            .path()
            .resolve("pdftotext", tauri::path::BaseDirectory::Resource)
            .map_err(|e| anyhow::anyhow!("could not resolve bundled pdftotext resource: {e}"))?;
        std::process::Command::new(&resource_path)
            .args(["-raw", path_str, "-"])
            .output()
            .map_err(|e| anyhow::anyhow!("pdftotext resource failed to run: {e}"))?
    };

    // Windows/macOS: no shared /usr/bin, so the collision above never
    // applies there — unchanged from before.
    #[cfg(not(target_os = "linux"))]
    let output = app
        .shell()
        .sidecar("pdftotext")
        .map_err(|e| anyhow::anyhow!("could not launch bundled pdftotext sidecar: {e}"))?
        .args(["-raw", path_str, "-"])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("pdftotext sidecar failed to run: {e}"))?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("pdftotext failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(parse_directory_text(&text))
}

fn parse_directory_text(text: &str) -> ParseResult {
    let lines = clean_lines(text);
    let name_re = name_line_re();

    let name_idx: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| name_re.is_match(l)).map(|(i, _)| i).collect();

    let mut records = Vec::new();
    let mut warnings = Vec::new();

    // Issue #56: every line from name_idx[0] onward falls inside some
    // block already (blocks run start..next-start..end-of-file), so the
    // only lines that can fall outside every block are the ones BEFORE
    // the first detected record start. Ordinarily that's zero — page
    // header/copyright boilerplate is stripped in clean_lines(). A
    // nonzero count here means something in the input wasn't recognized
    // as either boilerplate or a record start, which is exactly the
    // shape of gap this issue is about (a real record silently missing
    // no block at all, not just merged into a neighbour). Deliberately
    // does not fire on the existing fixture — see mod tests below.
    if let Some(&first) = name_idx.first() {
        if first > 0 {
            warnings.push(ParseWarning {
                context: lines[0].clone(),
                message: format!(
                    "{} line(s) before the first detected household were not recognized as boilerplate or a record start — possible parsing gap",
                    first
                ),
            });
        }
    } else if !lines.is_empty() {
        warnings.push(ParseWarning {
            context: lines[0].clone(),
            message: format!(
                "no household records detected in {} line(s) of input — possible parsing gap",
                lines.len()
            ),
        });
    }

    for (bi, &start) in name_idx.iter().enumerate() {
        let end = name_idx.get(bi + 1).copied().unwrap_or(lines.len());
        let block = &lines[start..end];

        let caps = match name_re.captures(&block[0]) {
            Some(c) => c,
            None => continue,
        };
        let last_name = caps[1].trim().to_string();
        let rest = caps[2].trim().to_string();
        let (names_part, glued_addr) = split_name_and_glued_address(&rest);
        let heads = parse_heads(&names_part);

        if heads.is_empty() {
            warnings.push(ParseWarning {
                context: block[0].clone(),
                message: "no head names could be parsed from this line".to_string(),
            });
            continue;
        }

        let mut remaining: Vec<String> = block[1..].to_vec();
        if let Some(addr) = glued_addr {
            remaining.insert(0, addr);
        }

        // ---- address (optional): look ahead for a City/State/Zip line ----
        let mut address_line1 = None;
        let mut address_line2 = None;
        let mut city = None;
        let mut state = None;
        let mut zip = None;

        let lookahead = remaining.len().min(4);
        let mut csz_offset = None;
        for k in 0..lookahead {
            let line = &remaining[k];
            if coord_re().is_match(line) || line.contains('@') || phone_re().is_match(line) {
                break;
            }
            if heads.iter().any(|h| name_matches_head(line, h.split(' ').next().unwrap_or(h))) {
                break;
            }
            if csz_re().is_match(line) {
                csz_offset = Some(k);
                break;
            }
        }
        if let Some(k) = csz_offset {
            let street_lines = &remaining[..k];
            if let Some(caps) = csz_re().captures(&remaining[k]) {
                city = Some(caps["city"].trim().to_string());
                state = Some(caps["state"].to_string());
                zip = Some(caps["zip"].to_string());
            }
            if !street_lines.is_empty() {
                address_line1 = Some(street_lines[0].clone());
            }
            if street_lines.len() >= 2 {
                address_line2 = Some(street_lines[1..].join("; "));
            }
            remaining = remaining[k + 1..].to_vec();
        }

        // Source uses a literal "0" as a null-address sentinel on some
        // records — drop it so a real coordinate line right after it is
        // still picked up below.
        if remaining.first().map(|l| l.trim()) == Some("0") {
            remaining.remove(0);
        }

        // ---- coordinates (optional) ----
        let mut latitude = None;
        let mut longitude = None;
        if let Some(first) = remaining.first() {
            if coord_re().is_match(first) {
                let parts: Vec<&str> = first.split(',').collect();
                let raw_lat: Option<f64> = parts.first().and_then(|s| s.trim().parse().ok());
                let raw_lon: Option<f64> = parts.get(1).and_then(|s| s.trim().parse().ok());
                // coord_re only checks shape (digits/dot/sign) — it happily
                // matches "999.999,-999.999", nowhere on Earth, and Rust's
                // f64::from_str accepts "NaN"/"inf" as valid floats too.
                // Neither is caught until this point. A rejected value is
                // dropped to NULL (already handled gracefully everywhere
                // downstream) rather than stored and later crashing
                // visit-list generation or the map (#24).
                match (
                    valid_coord(raw_lat, -90.0, 90.0),
                    valid_coord(raw_lon, -180.0, 180.0),
                ) {
                    (Some(lat), Some(lon)) => {
                        latitude = Some(lat);
                        longitude = Some(lon);
                    }
                    _ => {
                        warnings.push(ParseWarning {
                            context: format!("{last_name}, {names_part}"),
                            message: format!(
                                "coordinate '{first}' is missing, non-finite, or out of range — stored as no coordinates"
                            ),
                        });
                    }
                }
                remaining.remove(0);
            }
        }

        // ---- per-head name-repeat / phone / email lines — captured into
        // phone_1/email_1 (head 1) and phone_2/email_2 (head 2) ----
        let mut phones: Vec<Option<String>> = Vec::new();
        let mut emails: Vec<Option<String>> = Vec::new();
        for h in &heads {
            let first_tok = h.split(' ').next().unwrap_or(h);
            if remaining.first().map(|l| name_matches_head(l, first_tok)).unwrap_or(false) {
                remaining.remove(0);
            }
            let phone = if remaining.first().map(|l| phone_re().is_match(l)).unwrap_or(false) {
                Some(remaining.remove(0))
            } else {
                None
            };
            let email = if remaining.first().map(|l| l.contains('@') && !l.contains(' ')).unwrap_or(false) {
                Some(remaining.remove(0))
            } else {
                None
            };
            phones.push(phone);
            emails.push(email);
        }

        // ---- whatever's left is minors — flagged via has_minors only,
        // never stored (see ParsedRecord::has_minors doc comment). Still
        // surfaced as an import warning so it's visible at import time,
        // just not persisted to the database in any form.
        let leftover: Vec<String> = remaining.into_iter().filter(|l| !l.trim().is_empty()).collect();
        let has_minors = !leftover.is_empty();
        if has_minors {
            warnings.push(ParseWarning {
                context: format!("{last_name}, {names_part}"),
                message: "unparsed trailing line(s), likely minors — flagged but names not stored".to_string(),
            });
        }

        // ---- emit ONE record per entry, up to 2 co-heads ----
        // Real directory data confirmed: never more than 2 heads on a
        // single entry line. If a future source somehow has 3+, keep the
        // first 2 and push the rest into comments rather than silently
        // dropping people. This is a genuinely different case from
        // minors above — flagging an unexpected extra ADULT head, not a
        // child — so it still goes into comments as free text.
        let combined_comments = if heads.len() > 2 {
            warnings.push(ParseWarning {
                context: format!("{last_name}, {names_part}"),
                message: format!("entry lists {} heads, expected at most 2 — kept the first 2, rest added to comments", heads.len()),
            });
            Some(format!("Additional heads (unparsed, review manually): {}", heads[2..].join(", ")))
        } else {
            None
        };

        let (first_name, _) = split_first_middle(&heads[0]);
        let (first_name_2, last_name_2, role_2) = match heads.get(1) {
            Some(h2) => {
                let (full, _) = split_first_middle(h2);
                (Some(full), Some(last_name.clone()), Some("head".to_string()))
            }
            None => (None, None, None),
        };

        records.push(ParsedRecord {
            first_name,
            last_name: last_name.clone(),
            role: "head".to_string(),
            phone_1: phones.first().cloned().flatten(),
            email_1: emails.first().cloned().flatten(),
            first_name_2,
            last_name_2,
            role_2,
            phone_2: phones.get(1).cloned().flatten(),
            email_2: emails.get(1).cloned().flatten(),
            address_line1: address_line1.clone(),
            address_line2: address_line2.clone(),
            city: city.clone(),
            state: state.clone(),
            zip: zip.clone(),
            latitude,
            longitude,
            has_minors,
            comments: combined_comments,
        });
    }

    ParseResult { records, warnings }
}

/// Normalizes name+address for import dedupe matching: lowercase,
/// collapsed whitespace, punctuation stripped. Exact matches on this key
/// are discarded from the incoming batch as unchanged. Includes the
/// second head (if any) so a couple's key changes if either name does.
pub fn source_key(first: &str, last: &str, first2: &Option<String>, last2: &Option<String>, addr1: &Option<String>) -> String {
    normalize(&format!(
        "{} {} {} {} {}",
        first, last,
        first2.clone().unwrap_or_default(), last2.clone().unwrap_or_default(),
        addr1.clone().unwrap_or_default()
    ))
}

pub fn address_key(addr1: &Option<String>, addr2: &Option<String>, city: &Option<String>) -> String {
    normalize(&format!(
        "{} {} {}",
        addr1.clone().unwrap_or_default(),
        addr2.clone().unwrap_or_default(),
        city.clone().unwrap_or_default()
    ))
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/directory_sample.txt");

    #[test]
    fn parses_expected_record_count() {
        let result = parse_directory_text(SAMPLE);
        // One record per directory ENTRY now, not per person — couples
        // collapse to a single row (first_name_2/last_name_2 populated).
        assert_eq!(result.records.len(), 18, "warnings: {:?}", result.warnings);
    }

    #[test]
    fn issue_56_reconciliation_warning_does_not_fire_on_clean_fixture() {
        // Constraint: the added-lines-before-first-block check must stay
        // silent on this fixture, or every other test's warning-count
        // assumptions become noise.
        let result = parse_directory_text(SAMPLE);
        assert!(
            !result.warnings.iter().any(|w| w.message.contains("possible parsing gap")),
            "unexpected parsing-gap warning: {:?}",
            result.warnings
        );
    }

    #[test]
    fn handles_glued_name_and_address() {
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.last_name == "Winslow").unwrap();
        assert_eq!(r.first_name, "Vivian Skye");
        assert_eq!(r.address_line1.as_deref(), Some("410 Brookstone Rd"));
    }

    #[test]
    fn handles_null_address_sentinel_and_still_finds_coords() {
        let result = parse_directory_text(SAMPLE);
        // Matched by last_name, not first_name — "Grant" also appears as a
        // head on the unrelated Ferris entry, so first_name alone is
        // ambiguous in this fixture.
        let r = result.records.iter().find(|r| r.last_name == "Sinclair").unwrap();
        assert!(r.address_line1.is_none());
        assert_eq!(r.latitude, Some(39.158007));
    }

    #[test]
    fn couples_collapse_into_one_record_with_second_head() {
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.first_name == "Julian Micah").unwrap();
        assert_eq!(r.last_name, "Whitcombe");
        assert_eq!(r.first_name_2.as_deref(), Some("Fiona Grace"));
        assert_eq!(r.last_name_2.as_deref(), Some("Whitcombe"));
        assert_eq!(r.role_2.as_deref(), Some("head"));
    }

    #[test]
    fn single_head_entry_has_no_second_head() {
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.first_name == "Miles").unwrap();
        assert!(r.first_name_2.is_none());
        assert!(r.role_2.is_none());
    }

    #[test]
    fn separate_entry_at_same_address_stays_its_own_record() {
        // "Ferris, Grant & Monique" and "Ferris David, Sullivan" both
        // appear near "Ferris" but are genuinely two different directory
        // entries — must never be merged into one record. Two distinct
        // matches with the right shape (Grant has a second head, Sullivan
        // doesn't) proves they parsed as separate records.
        // Grant matched by last_name too — "Grant" alone is ambiguous,
        // it's also the first head of the unrelated Sinclair entry.
        let result = parse_directory_text(SAMPLE);
        let grant = result.records.iter().find(|r| r.first_name == "Grant" && r.last_name == "Ferris").unwrap();
        let sullivan = result.records.iter().find(|r| r.first_name == "Sullivan").unwrap();
        assert_eq!(grant.last_name, "Ferris");
        assert_eq!(sullivan.last_name, "Ferris David");
        assert!(grant.first_name_2.as_deref() == Some("Monique"));
        assert!(sullivan.first_name_2.is_none());
    }

    #[test]
    fn flags_minors_as_a_boolean_not_stored_names() {
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.first_name == "Julian Micah").unwrap();
        assert!(r.has_minors, "expected has_minors to be set");
        // The actual names must never end up in the database anywhere.
        assert!(!r.comments.as_deref().unwrap_or("").contains("Owen Julian"));
    }

    #[test]
    fn handles_multiword_lowercase_surname() {
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.last_name == "De La Cruz Montenegro").unwrap();
        assert_eq!(r.first_name, "Adriana Fernanda");
    }

    #[test]
    fn handles_unicode_uppercase_surname() {
        // Issue #56: [A-Z] used to be ASCII-only, so a surname starting
        // with Ö never registered as a block start at all — this asserts
        // the record exists as its own entry, not merged into a neighbour.
        let result = parse_directory_text(SAMPLE);
        let r = result.records.iter().find(|r| r.last_name == "Öhman")
            .unwrap_or_else(|| panic!("Öhman entry missing entirely — record was dropped"));
        assert_eq!(r.first_name, "Erik");
        assert_eq!(r.address_line1.as_deref(), Some("104 Aster Ln"));
        assert!(!r.has_minors);
    }

    #[test]
    fn accented_surname_does_not_corrupt_preceding_household() {
        // Issue #56: before the fix, the undetected "Öhman, Erik" block
        // was absorbed into the preceding household (Dana Halvorsen),
        // wrongly setting her has_minors flag from Öhman's own leftover
        // lines. Confirms the fix, not just that Öhman parses.
        let result = parse_directory_text(SAMPLE);
        let dana = result.records.iter().find(|r| r.first_name == "Dana").unwrap();
        assert!(!dana.has_minors, "Dana wrongly flagged has_minors — Öhman block was absorbed into her record");
    }

    #[test]
    fn does_not_drop_common_csz_line_as_boilerplate() {
        // Regression test: a frequency-based boilerplate filter used to
        // treat any line repeated 3+ times as a page header/footer and
        // strip it — which also nuked legitimate short city/state/zip
        // lines shared by unrelated households (very common in a real
        // directory). These three fixture records all share the exact
        // same "Somewhere UT 22602" line on purpose.
        let result = parse_directory_text(SAMPLE);
        for name in ["Nina", "Peter", "Dana"] {
            let r = result.records.iter().find(|r| r.first_name == name)
                .unwrap_or_else(|| panic!("{name} missing entirely — record was dropped"));
            assert_eq!(r.city.as_deref(), Some("Somewhere"), "{name}: city wrongly stripped");
            assert_eq!(r.state.as_deref(), Some("UT"), "{name}: state wrongly stripped");
        }
    }
}
