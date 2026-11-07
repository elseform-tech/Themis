//! Folding text, tokenizing queries and scanning fields for matches.

use super::*;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "did", "do", "does", "for", "from", "had",
    "has", "have", "he", "her", "his", "how", "i", "in", "is", "it", "its", "of", "on", "or",
    "she", "that", "the", "their", "them", "then", "there", "they", "this", "to", "was", "were",
    "what", "when", "where", "which", "who", "whom", "why", "will", "with", "you", "your",
];

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}')
}

/// Lowercase, accent-stripped form of a character; `None` drops combining marks.
pub fn fold_char(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c.to_ascii_lowercase());
    }
    if matches!(c, '\u{300}'..='\u{36f}' | '\u{200b}'..='\u{200d}' | '\u{feff}') {
        return None;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    Some(strip_accent(lower))
}

fn strip_accent(c: char) -> char {
    const TABLE: &[(&str, char)] = &[
        ("àáâãäåāăąǎ", 'a'),
        ("çćĉċč", 'c'),
        ("ďđ", 'd'),
        ("èéêëēĕėęě", 'e'),
        ("ĝğġģ", 'g'),
        ("ĥħ", 'h'),
        ("ìíîïĩīĭįıǐ", 'i'),
        ("ĵ", 'j'),
        ("ķ", 'k'),
        ("ĺļľŀł", 'l'),
        ("ñńņňŉ", 'n'),
        ("òóôõöøōŏőǒ", 'o'),
        ("ŕŗř", 'r'),
        ("śŝşšș", 's'),
        ("ţťŧț", 't'),
        ("ùúûüũūŭůűųǔ", 'u'),
        ("ŵ", 'w'),
        ("ýÿŷ", 'y'),
        ("źżž", 'z'),
    ];
    TABLE
        .iter()
        .find(|(accented, _)| accented.contains(c))
        .map_or(c, |(_, base)| *base)
}

/// Folded characters (non-alphanumerics become spaces) plus each one's
/// original character index.
fn fold_text(chars: &[char]) -> (Vec<char>, Vec<u32>) {
    let mut folded = Vec::with_capacity(chars.len());
    let mut map = Vec::with_capacity(chars.len());
    for (index, c) in chars.iter().enumerate() {
        if let Some(c) = fold_char(*c) {
            folded.push(if c.is_alphanumeric() { c } else { ' ' });
            map.push(index as u32);
        }
    }
    (folded, map)
}

/// Token ranges (folded indices) of a folded character run.
fn tokens(folded: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, c) in folded.iter().enumerate() {
        if *c == ' ' {
            if let Some(s) = start.take() {
                out.push((s, index));
            }
        } else if is_cjk(*c) {
            if let Some(s) = start.take() {
                out.push((s, index));
            }
            out.push((index, index + 1));
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(s) = start {
        out.push((s, folded.len()));
    }
    out
}

fn word_matches(word: &[char], term: &[char]) -> bool {
    word == term || (term.len() >= 4 && word.len() > term.len() && word.starts_with(term))
}

/// A folded query: the phrase and the distinct scored terms.
#[derive(Debug, Clone)]
pub struct Query {
    phrase: Vec<Vec<char>>,
    /// Distinct scored terms (stopwords are skipped unless nothing else remains).
    pub terms: Vec<Vec<char>>,
}

impl Query {
    /// Folds `text` into a phrase and scored terms.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().take(2000).collect();
        let (folded, _) = fold_text(&chars);
        let mut phrase: Vec<Vec<char>> = tokens(&folded)
            .into_iter()
            .map(|(s, e)| folded[s..e].to_vec())
            .collect();
        phrase.truncate(MAX_PHRASE);
        let is_stop = |word: &Vec<char>| {
            let word: String = word.iter().collect();
            STOPWORDS.contains(&word.as_str())
        };
        let mut terms: Vec<Vec<char>> = Vec::new();
        for word in phrase.iter().filter(|word| !is_stop(word)) {
            if !terms.contains(word) && terms.len() < MAX_TERMS {
                terms.push(word.clone());
            }
        }
        if terms.is_empty() {
            for word in &phrase {
                if !terms.contains(word) && terms.len() < MAX_TERMS {
                    terms.push(word.clone());
                }
            }
        }
        Self { phrase, terms }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Occurrence {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) term: usize,
}

#[derive(Debug, Default)]
pub(super) struct FieldScan {
    pub(super) occurrences: Vec<Occurrence>,
    pub(super) phrases: Vec<(usize, usize)>,
    /// More matches existed than were kept.
    pub(super) capped: bool,
    /// The deadline passed before the whole field was scanned.
    pub(super) timed_out: bool,
}

/// Finds term and phrase occurrences in `text`, scanning long text in
/// overlapping chunks so memory stays bounded. Both match lists are capped at
/// [`MAX_OCCURRENCES`] (`capped`), and `deadline` stops between chunks.
pub(super) fn scan_text(text: &str, query: &Query, deadline: Option<Instant>) -> FieldScan {
    let mut scan = FieldScan::default();
    let mut byte_start = 0usize;
    let mut abs_start = 0usize;
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            scan.timed_out = true;
            break;
        }
        let rest = &text[byte_start..];
        let mut chars = Vec::new();
        let mut next_byte = None;
        let mut consumed = 0usize;
        for (offset, c) in rest.char_indices() {
            if chars.len() == CHUNK_CHARS {
                break;
            }
            if chars.len() == CHUNK_CHARS - CHUNK_OVERLAP {
                next_byte = Some(offset);
            }
            chars.push(c);
            consumed = offset + c.len_utf8();
        }
        let last = consumed >= rest.len();
        let low = if abs_start == 0 { 0 } else { CHUNK_OVERLAP / 2 };
        let high = if last {
            chars.len()
        } else {
            chars.len() - CHUNK_OVERLAP / 2
        };
        let (folded, map) = fold_text(&chars);
        let spans = tokens(&folded);
        for (token, (s, e)) in spans.iter().enumerate() {
            let start = map[*s] as usize;
            if start < low || start >= high {
                continue;
            }
            let end = map[e - 1] as usize + 1;
            for (term, wanted) in query.terms.iter().enumerate() {
                if word_matches(&folded[*s..*e], wanted) {
                    if scan.occurrences.len() < MAX_OCCURRENCES {
                        scan.occurrences.push(Occurrence {
                            start: abs_start + start,
                            end: abs_start + end,
                            term,
                        });
                    } else {
                        scan.capped = true;
                    }
                }
            }
            let words = query.phrase.len();
            if words > 0
                && token + words <= spans.len()
                && (0..words).all(|offset| {
                    let (s, e) = spans[token + offset];
                    word_matches(&folded[s..e], &query.phrase[offset])
                })
            {
                if scan.phrases.len() < MAX_OCCURRENCES {
                    let (_, last_end) = spans[token + words - 1];
                    scan.phrases.push((
                        abs_start + start,
                        abs_start + map[last_end - 1] as usize + 1,
                    ));
                } else {
                    scan.capped = true;
                }
            }
        }
        match (last, next_byte) {
            (false, Some(next)) => {
                byte_start += next;
                abs_start += CHUNK_CHARS - CHUNK_OVERLAP;
            }
            _ => break,
        }
    }
    scan.occurrences.sort_by_key(|o| (o.start, o.term));
    scan.occurrences.dedup_by_key(|o| (o.start, o.term));
    scan.phrases.sort_unstable();
    scan.phrases.dedup();
    scan
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Window {
    pub(super) tier: u8,
    pub(super) distinct: usize,
    /// Bit per query term matched inside the passage.
    pub(super) mask: u32,
    pub(super) start: usize,
    pub(super) end: usize,
}

/// Up to [`WINDOWS_PER_FIELD`] best non-overlapping passages of a field.
pub(super) fn best_windows(scan: &FieldScan, terms: usize) -> Vec<Window> {
    let mut used: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    let occurrences = &scan.occurrences;
    let overlaps = |used: &[(usize, usize)], start: usize, end: usize| {
        used.iter().any(|(s, e)| start < *e && *s < end)
    };
    for _ in 0..WINDOWS_PER_FIELD {
        let mut best: Option<Window> = None;
        for (i, first) in occurrences.iter().enumerate() {
            if overlaps(&used, first.start, first.end) {
                continue;
            }
            let mut mask = 0u32;
            let mut end = first.end;
            for (j, other) in occurrences[i..].iter().enumerate() {
                if j > 0 && other.end.saturating_sub(first.start) > WINDOW_SPAN {
                    break;
                }
                if !overlaps(&used, other.start, other.end) {
                    mask |= 1 << other.term;
                    end = end.max(other.end);
                }
            }
            let distinct = mask.count_ones() as usize;
            let phrase = scan
                .phrases
                .iter()
                .find(|(ps, pe)| *ps < end && *pe > first.start && !overlaps(&used, *ps, *pe));
            let (tier, start, end) = match phrase {
                Some((ps, pe)) => (3u8, first.start.min(*ps), end.max(*pe)),
                None if distinct == terms => (2, first.start, end),
                None => (1, first.start, end),
            };
            let candidate = Window {
                tier,
                distinct,
                mask,
                start,
                end,
            };
            if best.is_none_or(|b| (candidate.tier, candidate.distinct) > (b.tier, b.distinct)) {
                best = Some(candidate);
            }
        }
        let Some(window) = best else { break };
        used.push((window.start, window.end));
        out.push(window);
    }
    out
}
