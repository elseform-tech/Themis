//! Whether a reply with no tool call really ends the turn.
//!
//! The runtime reads a reply without a tool call as the end of the turn. Free models often
//! stop on a line that only announces the next step, or write a tool call out as text, and
//! the work is lost. Such a reply gets one more request per stretch of work (the replies
//! between two tool calls), so a model that keeps doing it cannot loop.
//!
//! Replies are untrusted model text. A closing summary and an offer to the person must end
//! the turn, so the reading leans towards ending it when unsure.

/// Why a reply with no tool call does not end the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unfinished {
    /// A short line that says the work is still going ([`super::progress_line`]).
    ProgressLine,
    /// The last sentence announces a step the reply did not take.
    Announced,
    /// The reply holds a tool call written out as text.
    WrittenCall,
}

impl Unfinished {
    /// The name diagnostics record it under.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ProgressLine => "progress_line",
            Self::Announced => "announced",
            Self::WrittenCall => "written_call",
        }
    }
}

/// Markup models use for tool calls, which no plain reply holds outside code.
const CALL_MARKUP: [&str; 8] = [
    "<tool_call",
    "</tool_call",
    "<function=",
    "<function_calls",
    "<invoke",
    "<tool_use",
    "<minimax:tool_call",
    "[tool_calls]",
];
/// Words naming a special token that carries a tool call, unlike `<|im_end|>`.
const CALL_TOKEN_WORDS: [&str; 7] = [
    "tool",
    "func",
    "call",
    "invoke",
    "python_tag",
    "channel",
    "arg",
];
/// Words that hand the turn to the person, or stop it.
const HANDED_OVER: [&str; 9] = [
    "let me know",
    "let us know",
    "point me",
    "tell me",
    "ask me",
    "want me to",
    "should i",
    "shall i",
    " for your ",
];
/// What follows "I'll" when nothing more is coming: "I'll leave it there."
const STOPPING_VERBS: [&str; 11] = [
    "leave", "wait", "stop", "be", "await", "hold", "pause", "end", "know", "hear", "ask",
];
/// How a sentence says the speaker is about to act, anywhere in it.
const ABOUT_TO: [&str; 7] = [
    " i'll ",
    " i will ",
    " i'm going to ",
    " i am going to ",
    " i need to ",
    " we'll ",
    " we will ",
];
/// Openers that only pace a sentence: "Now let's run the tests."
const FILLERS: [&str; 7] = ["now", "next", "then", "so", "okay", "ok", "alright"];
/// Verbs after "Now I" that report a state and not a step: "Now I see the problem."
const STATE_VERBS: [&str; 7] = ["see", "know", "understand", "have", "am", "think", "can"];
/// Opening words ending in -ing that do not announce an action.
const NOT_STEPS: [&str; 16] = [
    "everything",
    "nothing",
    "something",
    "anything",
    "being",
    "during",
    "following",
    "including",
    "regarding",
    "remaining",
    "missing",
    "pending",
    "leaving",
    "stopping",
    "ending",
    "pausing",
];
/// A second word that turns an opening verb into a report: "Looking good.", "Working as expected."
const SETTLED: [&str; 11] = [
    "good",
    "fine",
    "great",
    "well",
    "correctly",
    "as",
    "now",
    "again",
    "perfectly",
    "ok",
    "okay",
];
/// Verbs that make an opening -ing word the subject of a report: "Testing confirmed the fix."
const REPORTING: [&str; 22] = [
    "is",
    "was",
    "are",
    "were",
    "has",
    "had",
    "have",
    "does",
    "did",
    "fixed",
    "confirmed",
    "showed",
    "shows",
    "works",
    "worked",
    "passes",
    "passed",
    "solved",
    "failed",
    "fails",
    "found",
    "proved",
];
/// Words after which a verb belongs to a clause of the step: "Checking why the test failed."
const CLAUSE_WORDS: [&str; 11] = [
    "why", "what", "whether", "because", "if", "that", "since", "until", "how", "where", "to",
];

/// How a reply with no tool call reads. `worked` says whether this run has used tools yet.
pub(super) fn unfinished_ending(text: &str, worked: bool) -> Option<Unfinished> {
    if writes_tool_call(text) {
        return Some(Unfinished::WrittenCall);
    }
    let lower = text.trim().to_lowercase().replace('\u{2019}', "'");
    let closed = lower.trim_end_matches(['*', '_', '"', ' ', '\n']);
    let body = closed.trim_end_matches(['.', '!', '\u{2026}', ' ', '*', '_', '"', '\n']);
    let sentence = last_sentence(body);
    if body.ends_with('?') || hands_over(&sentence) {
        return None;
    }
    if !ends_on_list_item(body) && announces_step(&sentence, closed, worked) {
        return Some(Unfinished::Announced);
    }
    (worked && super::progress_line(text)).then_some(Unfinished::ProgressLine)
}

/// The last sentence. One ends at a mark followed by a space, so `notes/b.txt` stays whole.
fn last_sentence(body: &str) -> String {
    body.replace(". ", "\n")
        .replace("! ", "\n")
        .replace("? ", "\n")
        .lines()
        .map(|line| line.trim().trim_start_matches(['*', '_']))
        .rfind(|sentence| !sentence.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// A closing list reports what was done, often in -ing form ("- Adding the regression test").
fn ends_on_list_item(body: &str) -> bool {
    let line = body
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let marker = line.split_whitespace().next().unwrap_or_default();
    let numbered = marker
        .strip_suffix(['.', ')'])
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()));
    matches!(marker, "-" | "*" | "+") || numbered
}

/// "Once I have those results, I'll let you know" names work still to do before it reports,
/// unlike "When you're ready, I'll start", which waits on the person.
fn waits_on_own_work(sentence: &str, spaced: &str) -> bool {
    let Some((opening, _)) = sentence.split_once(',') else {
        return false;
    };
    ["once ", "when ", "after "]
        .iter()
        .any(|start| opening.starts_with(start))
        && !opening
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "you")
        && ABOUT_TO.iter().any(|about| spaced.contains(about))
}

/// An offer or hand-off to the person ("want me to...", "I'll leave that for you") is no
/// unfinished step, whichever way it is worded.
fn hands_over(sentence: &str) -> bool {
    let spaced = format!(" {sentence} ");
    if waits_on_own_work(sentence, &spaced) {
        return false;
    }
    sentence
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "you")
        || HANDED_OVER.iter().any(|words| spaced.contains(words))
        || ABOUT_TO.iter().any(|about| {
            spaced.split(about).skip(1).any(|after| {
                let next = after.split_whitespace().next().unwrap_or_default();
                STOPPING_VERBS.contains(&next)
            })
        })
}

fn announces_step(sentence: &str, reply: &str, worked: bool) -> bool {
    let spaced = format!(" {sentence} ");
    if ABOUT_TO.iter().any(|about| spaced.contains(about)) {
        return true;
    }
    let mut paced = sentence;
    while let Some(rest) = FILLERS.iter().find_map(|filler| {
        let rest = paced.strip_prefix(filler)?;
        rest.starts_with([' ', ',']).then_some(rest)
    }) {
        paced = rest.trim_start_matches([' ', ',']);
    }
    if ["let me ", "let us ", "let's "]
        .iter()
        .any(|start| paced.starts_with(start))
    {
        return true;
    }
    if paced != sentence {
        if let Some(rest) = paced.strip_prefix("i ") {
            let verb = rest.split_whitespace().next().unwrap_or_default();
            return !STATE_VERBS.contains(&verb);
        }
    }
    opens_with_step(paced, reply, worked)
}

/// Whether a sentence opens on a verb in -ing form saying what happens now ("Moving view to
/// line up with that enemy now."). Before any tool ran, plain answers read the same way
/// ("Using Python 3."), so it also takes a word that places the step in the present.
fn opens_with_step(sentence: &str, reply: &str, worked: bool) -> bool {
    let mut words = sentence.split_whitespace();
    let first = words.next().unwrap_or_default().trim_end_matches(',');
    if first.len() <= 4 || !first.ends_with("ing") || NOT_STEPS.contains(&first) {
        return false;
    }
    let trailing_off = reply.ends_with('\u{2026}') || reply.ends_with("...");
    let rest: Vec<&str> = words.collect();
    let Some(second) = rest.first() else {
        return trailing_off;
    };
    if SETTLED.contains(&second.trim_end_matches(',')) {
        return false;
    }
    let reports = rest
        .iter()
        .take_while(|word| !CLAUSE_WORDS.contains(word))
        .any(|word| REPORTING.contains(word));
    !reports && (worked || trailing_off || matches!(rest.last(), Some(&"now" | &"next")))
}

/// How far ahead a special token's closing may be: a longer one isn't a token.
const CALL_TOKEN_REACH: usize = 64;

/// Holds back what could still turn out to be a tool call written as text, and once one starts
/// shows nothing more of that reply: models follow one with their private planning. Text inside
/// code is someone talking about markup, so it passes. Reads each byte once.
#[derive(Default)]
pub(super) struct WrittenCallFilter {
    held: String,
    in_fence: bool,
    in_span: bool,
    cut: bool,
}

/// What the text at the start of `rest` could be.
enum Lookahead {
    Plain,
    NeedsMore,
    Call,
}

fn look_at_markup(rest: &str, at_end: bool) -> Lookahead {
    let lower = rest
        .chars()
        .take(CALL_TOKEN_REACH)
        .collect::<String>()
        .to_lowercase();
    if !lower.starts_with('<') && !lower.starts_with('[') {
        return Lookahead::Plain;
    }
    if CALL_MARKUP.iter().any(|mark| lower.starts_with(mark)) {
        return Lookahead::Call;
    }
    for (open, close) in [("<|", "|>"), ("<\u{ff5c}", "\u{ff5c}>")] {
        if let Some(after) = lower.strip_prefix(open) {
            return match after.split_once(close) {
                Some((name, _)) if CALL_TOKEN_WORDS.iter().any(|word| name.contains(word)) => {
                    Lookahead::Call
                }
                Some(_) => Lookahead::Plain,
                None if at_end || lower.chars().count() >= CALL_TOKEN_REACH => Lookahead::Plain,
                None => Lookahead::NeedsMore,
            };
        }
    }
    let could_grow = CALL_MARKUP
        .iter()
        .any(|mark| mark.starts_with(lower.as_str()))
        || "<|".starts_with(lower.as_str())
        || "<\u{ff5c}".starts_with(lower.as_str());
    if could_grow && !at_end {
        Lookahead::NeedsMore
    } else {
        Lookahead::Plain
    }
}

impl WrittenCallFilter {
    /// What can be shown now of a reply's next piece.
    pub(super) fn push(&mut self, delta: &str) -> String {
        if self.cut {
            return String::new();
        }
        self.held.push_str(delta);
        self.drain(false)
    }

    /// The end of the reply: what was held back wasn't a call after all.
    pub(super) fn finish(&mut self) -> String {
        if self.cut {
            return String::new();
        }
        self.drain(true)
    }

    fn drain(&mut self, at_end: bool) -> String {
        let text = std::mem::take(&mut self.held);
        let mut at = 0;
        while let Some(next) = text[at..].chars().next() {
            let rest = &text[at..];
            if next == '`' {
                // A run of ticks may still be growing; it decides what it opens or closes whole.
                let ticks = rest.chars().take_while(|tick| *tick == '`').count();
                if ticks == rest.len() && !at_end {
                    break;
                }
                if ticks >= 3 {
                    self.in_fence = !self.in_fence;
                } else if !self.in_fence && ticks == 1 {
                    self.in_span = !self.in_span;
                }
                at += ticks;
                continue;
            }
            if !self.in_fence && !self.in_span {
                match look_at_markup(rest, at_end) {
                    Lookahead::Call => {
                        self.cut = true;
                        let shown = text[..at].to_owned();
                        return shown;
                    }
                    Lookahead::NeedsMore => break,
                    Lookahead::Plain => {}
                }
            }
            at += next.len_utf8();
        }
        self.held = text[at..].to_owned();
        let mut shown = text;
        shown.truncate(at);
        shown
    }
}

/// A whole reply as the person sees it.
pub(super) fn before_written_call(text: &str) -> String {
    let mut filter = WrittenCallFilter::default();
    let mut shown = filter.push(text);
    shown.push_str(&filter.finish());
    shown
}

/// Whether a reply holds a tool call written out as text, and not just talk about one.
pub(super) fn writes_tool_call(text: &str) -> bool {
    let prose = outside_code(text).to_lowercase();
    CALL_MARKUP.iter().any(|mark| prose.contains(mark))
        || has_call_token(&prose, "<|", "|>")
        || has_call_token(&prose, "<\u{ff5c}", "\u{ff5c}>")
        || ends_in_json_call(text)
}

/// The text without fenced blocks and inline code spans.
fn outside_code(text: &str) -> String {
    text.split("```")
        .step_by(2)
        .flat_map(|part| part.split('`').step_by(2))
        .collect::<Vec<_>>()
        .join(" ")
}

fn has_call_token(prose: &str, open: &str, close: &str) -> bool {
    let mut rest = prose;
    while let Some((_, after_open)) = rest.split_once(open) {
        let Some((name, after_close)) = after_open.split_once(close) else {
            return false;
        };
        if CALL_TOKEN_WORDS.iter().any(|word| name.contains(word)) {
            return true;
        }
        rest = after_close;
    }
    false
}

/// A reply that is, or ends in, a JSON object naming a tool and its arguments.
fn ends_in_json_call(text: &str) -> bool {
    let mut body = text.trim();
    if let Some(fenced) = body.strip_suffix("```") {
        let Some((_, inside)) = fenced.rsplit_once("```") else {
            return false;
        };
        // What follows the opening fence on its line is the language name.
        body = inside
            .split_once('\n')
            .map_or(inside, |(_, code)| code)
            .trim();
    }
    if !body.ends_with(['}', ']']) {
        return false;
    }
    let last_line = body.rfind('\n').map_or(0, |at| at + 1);
    [
        Some(0),
        body.find('{'),
        body[last_line..].find('{').map(|at| at + last_line),
    ]
    .into_iter()
    .flatten()
    .any(|start| {
        serde_json::from_str::<serde_json::Value>(&body[start..])
            .is_ok_and(|value| names_a_call(&value))
    })
}

fn names_a_call(value: &serde_json::Value) -> bool {
    let named = |value: &serde_json::Value| value["name"].is_string();
    match value {
        serde_json::Value::Array(items) => items.iter().any(names_a_call),
        serde_json::Value::Object(_) => {
            names_a_call(&value["tool_calls"])
                || (named(&value["function"])
                    && (value["function"]["arguments"].is_object()
                        || value["function"]["arguments"].is_string()))
                || (named(value)
                    && ["arguments", "parameters", "args", "input"]
                        .iter()
                        .any(|field| value[*field].is_object()))
        }
        _ => false,
    }
}

/// Allows one extra request per stretch of work.
#[derive(Default)]
pub(super) struct TurnEndGuard {
    asked: bool,
}

impl TurnEndGuard {
    /// The reason to send this reply back, at most once between two tool calls.
    pub(super) fn check(&mut self, text: &str, worked: bool) -> Option<Unfinished> {
        if self.asked {
            return None;
        }
        let reason = unfinished_ending(text, worked);
        self.asked = reason.is_some();
        reason
    }

    /// A tool ran: what follows is a new stretch of work.
    pub(super) fn tool_ran(&mut self) {
        self.asked = false;
    }
}

enum RepeatState {
    /// Everything so far could still be the line again, so it is held back.
    Matching,
    /// The line came again: the space after it goes with it.
    SkippingSpace,
    Passing,
}

/// Keeps the reply to the guard's note from showing again what the person already saw, and
/// starts what is new in its own paragraph. A reply with nothing new shows nothing at all.
pub(super) struct RepeatFilter {
    shown: String,
    held: String,
    state: RepeatState,
    break_due: bool,
}

impl RepeatFilter {
    pub(super) fn new(shown: &str) -> Self {
        Self {
            shown: shown.trim().to_owned(),
            held: String::new(),
            state: RepeatState::Matching,
            break_due: true,
        }
    }

    /// A retried attempt starts its text over; a paragraph break already sent stays sent.
    pub(super) fn restart(&mut self) {
        self.held.clear();
        self.state = RepeatState::Matching;
    }

    fn show(&mut self, text: &str) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        Some(if std::mem::take(&mut self.break_due) {
            format!("\n\n{text}")
        } else {
            text.to_owned()
        })
    }

    /// The part of a delta to show now, if any.
    pub(super) fn push(&mut self, delta: &str) -> Option<String> {
        match self.state {
            RepeatState::Passing => self.show(delta),
            RepeatState::SkippingSpace => {
                let text = delta.trim_start();
                if !text.is_empty() {
                    self.state = RepeatState::Passing;
                }
                self.show(text)
            }
            RepeatState::Matching => {
                self.held.push_str(delta);
                let held = std::mem::take(&mut self.held);
                let candidate = held.trim_start();
                if let Some(rest) = candidate.strip_prefix(self.shown.as_str()) {
                    let rest = rest.trim_start();
                    self.state = if rest.is_empty() {
                        RepeatState::SkippingSpace
                    } else {
                        RepeatState::Passing
                    };
                    self.show(rest)
                } else if self.shown.starts_with(candidate) {
                    self.held = held;
                    None
                } else {
                    self.state = RepeatState::Passing;
                    self.show(candidate)
                }
            }
        }
    }

    /// What was held back when the reply ends, if it should be shown after all. The line
    /// again without its closing mark is still the line.
    pub(super) fn finish(&mut self) -> Option<String> {
        const CLOSING: [char; 3] = ['.', '!', '\u{2026}'];
        let held = std::mem::take(&mut self.held);
        let candidate = held.trim_start();
        let same =
            candidate.trim_end().trim_end_matches(CLOSING) == self.shown.trim_end_matches(CLOSING);
        if same {
            None
        } else {
            self.show(candidate)
        }
    }
}

#[cfg(test)]
mod tests {
    /// As a free model really sent it: a call's text, then its planning. None of it is for the person.
    const LEAKED: &str = "Committed both.\n\n<|func_arg_name|>args<|func_arg_value|>[\"git\"]\n\nAlso second command\nAnother shell call:";

    fn shown_in_pieces(text: &str, size: usize) -> String {
        let mut filter = WrittenCallFilter::default();
        let mut shown = String::new();
        let chars: Vec<char> = text.chars().collect();
        for piece in chars.chunks(size.max(1)) {
            shown.push_str(&filter.push(&piece.iter().collect::<String>()));
        }
        shown.push_str(&filter.finish());
        shown
    }

    #[test]
    fn a_written_call_and_what_follows_it_never_reach_the_person() {
        for size in [1, 2, 3, 7, 1000] {
            assert_eq!(
                shown_in_pieces(LEAKED, size),
                "Committed both.\n\n",
                "{size}"
            );
        }
        for text in [
            "Reading it now. <|tool_call_begin|>read_file<|tool_call_end|>",
            "Reading it now. <tool_call>{\"name\":\"read_file\"}</tool_call>",
            "Reading it now. <function=read_file>",
            "Reading it now. [TOOL_CALLS] x",
            "Reading it now. <\u{ff5c}tool\u{ff5c}> x",
        ] {
            assert_eq!(shown_in_pieces(text, 1), "Reading it now. ", "{text}");
        }
        assert_eq!(before_written_call(LEAKED), "Committed both.\n\n");
    }

    #[test]
    fn talk_about_markup_and_ordinary_text_pass_untouched() {
        for text in [
            "I stripped the `<tool_call>` tags from the transcript.",
            "The stop token is <|im_end|>, which the parser now drops.",
            "Fixed the parser.\n```\n<|tool_call_begin|>\n```\nIt no longer leaks.",
            "x < y and a <b> tag, 3 <| 4 and [links](a) with [tool] notes.",
            "Ends on a partial <",
            "Ends on a partial <|too",
            "Ends on a tick `",
        ] {
            for size in [1, 2, 3, 1000] {
                assert_eq!(shown_in_pieces(text, size), text, "{size}: {text}");
            }
        }
    }

    #[test]
    fn the_written_call_flag_outlives_stripping() {
        assert!(writes_tool_call(LEAKED));
        assert!(!writes_tool_call(&before_written_call(LEAKED)));
    }

    proptest! {
        /// However it arrives, what's shown is the start of what was sent, and the same whatever
        /// the pieces; it never shows a call a whole reply wouldn't.
        #[test]
        fn pieces_do_not_change_what_is_shown(
            text in "[a-z <|>`\\[\\]_]{0,80}",
            call in proptest::sample::select(vec!["", "<|tool_call|>", "<tool_call>", "<|func_arg_name|>"]),
            tail in "[a-z ]{0,20}",
            size in 1usize..9,
        ) {
            let whole = format!("{text}{call}{tail}");
            let shown = shown_in_pieces(&whole, size);
            prop_assert!(whole.starts_with(&shown));
            prop_assert_eq!(&shown, &shown_in_pieces(&whole, 10_000));
            prop_assert!(!writes_tool_call(&shown) || !shown.contains(call) || call.is_empty());
        }
    }

    use super::*;
    use proptest::prelude::*;

    const SHOWN: &str = "Let us aim and shoot the damaged enemy.";

    /// Every example from the free-model goal run, and the closers it must not catch.
    #[test]
    fn a_reply_that_only_announces_a_step_gets_another_request() {
        for worked in [false, true] {
            for reply in [
                "Let us aim and shoot the damaged enemy.",
                "Moving view to line up with that enemy now.",
                "Let me check the console next.",
            ] {
                assert_eq!(
                    unfinished_ending(reply, worked),
                    Some(Unfinished::Announced),
                    "{reply}"
                );
            }
        }
    }

    /// Found in a live run: after subagents reported back, the model promised to check their
    /// work and stopped. "Let you know" is not handing over when it waits on work still to do.
    #[test]
    fn a_promise_to_report_after_more_work_gets_another_request() {
        for worked in [false, true] {
            for reply in [
                "So I'll read both files directly from the project root right now to confirm the actual contents. Once I have those results, I'll let you know for sure what I found.",
                "Once I have those results, I'll let you know.",
                "When the tests finish, I will tell you what they show.",
                "After I check the files, I'll report back.",
            ] {
                assert_eq!(
                    unfinished_ending(reply, worked),
                    Some(Unfinished::Announced),
                    "{reply}"
                );
            }
        }
    }

    #[test]
    fn a_closing_reply_ends_the_turn() {
        for worked in [false, true] {
            for reply in [
                "I fixed the overlay and the score now rises.",
                "Done. Let me know if you need anything else.",
                "I'll leave that for you to decide.",
                "Once you have tried it, let me know how it goes.",
                "When you're ready, I'll start on the next part.",
                "If you want, I'll let you know when it ships.",
            ] {
                assert_eq!(unfinished_ending(reply, worked), None, "{reply}");
            }
        }
    }

    /// The cases #164 settled on, on either side of the line.
    #[test]
    fn announcements_at_any_length_are_caught() {
        for reply in [
            "Confirmed - notes/a.txt contains \"hello\".\n\nSending the Writer back to create notes/b.txt.",
            "All good so far. I'll send it back now.",
            "Looks wrong. Let me check the file.",
            "Checking the other folder\u{2026}",
            "Good so far. If any line fails the rule, I'll send it back to Writer.",
            "That didn\u{2019}t work. I\u{2019}ll try the other path.",
            "**Moving view to line up with that enemy now.**",
            "Now let's run the tests.",
            "Checking why the test failed.",
            "Let me check your console.",
        ] {
            assert_eq!(
                unfinished_ending(reply, true),
                Some(Unfinished::Announced),
                "{reply}"
            );
        }
    }

    #[test]
    fn offers_questions_and_finished_work_are_not_steps() {
        for reply in [
            "Fixed add.py and the test passes.",
            "Everything passes.",
            "Nothing else is left.",
            "It passes. Let me know if you want more.",
            "If they are elsewhere, point me to the path and I'll take care of it!",
            "I'll add it if you want.",
            "Should I send it back?",
            "Want me to run the tests?",
            "Want me to run the tests next.",
            "Writer created both files, and I read them to confirm.",
            "I'll wait for your answer.",
            "I'll stop here.",
            "I'll leave it there.",
            "Waiting for your reply.",
            "Looking good.",
            "Working as expected.",
            "Testing confirmed the fix.",
            "Adding the guard fixed it.",
            "Passing.",
            "- Updated the docs\n- Adding the regression test",
            "Let us know if it breaks.",
            "Now I see the problem: the path was wrong.",
        ] {
            assert_eq!(unfinished_ending(reply, true), None, "{reply}");
        }
    }

    /// Before any tool ran, a bare opening verb is too weak: plain answers read like it.
    #[test]
    fn a_plain_answer_is_not_read_as_a_step() {
        for reply in ["Using Python 3.", "Doing well, thanks!", "4", "Yes."] {
            assert_eq!(unfinished_ending(reply, false), None, "{reply}");
        }
        assert_eq!(
            unfinished_ending("Sending it back.", true),
            Some(Unfinished::Announced)
        );
        assert_eq!(unfinished_ending("Continuing.", false), None);
    }

    /// A short line that says the work goes on still counts, unless it is an offer.
    #[test]
    fn progress_lines_still_count_after_tools() {
        for line in [
            "Silver-sorrel corrected, moving on to silver-zephyr.",
            "Continuing.",
        ] {
            assert_eq!(
                unfinished_ending(line, true),
                Some(Unfinished::ProgressLine),
                "{line}"
            );
        }
    }

    #[test]
    fn a_tool_call_written_as_text_gets_another_request() {
        for reply in [
            "Reading it now. <|func_arg_name|>file_path",
            "<|tool_call_begin|>read_file<|tool_call_end|>",
            "<\u{ff5c}tool\u{2581}calls\u{2581}begin\u{ff5c}>",
            "<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}</tool_call>",
            "I fixed it. <function=read_file>{\"path\":\"a\"}</function>",
            "<invoke name=\"read_file\"></invoke>",
            "[TOOL_CALLS] [{\"name\":\"read_file\",\"arguments\":{}}]",
            "{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}",
            "Reading it.\n{\"name\":\"read_file\",\"parameters\":{\"path\":\"a\"}}",
            "```json\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}\n```",
            "{\"tool_calls\":[{\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]}",
        ] {
            for worked in [false, true] {
                assert_eq!(
                    unfinished_ending(reply, worked),
                    Some(Unfinished::WrittenCall),
                    "{reply}"
                );
            }
        }
    }

    /// A reply that talks about markup or data has made no call.
    #[test]
    fn talk_about_tool_markup_is_not_a_call() {
        for reply in [
            "I stripped the `<tool_call>` tags from the transcript.",
            "The stop token is <|im_end|>, which the parser now drops.",
            "Fixed the parser.\n```\n<|tool_call_begin|>\n```\nIt no longer leaks.",
            "The file holds {\"name\":\"a\",\"size\":3}.",
            "{\"name\":\"a\",\"arguments\":3}",
            "a < b and b > c.",
        ] {
            assert_eq!(unfinished_ending(reply, true), None, "{reply}");
        }
    }

    #[test]
    fn the_guard_fires_once_until_a_tool_runs() {
        let mut guard = TurnEndGuard::default();
        assert_eq!(guard.check(SHOWN, true), Some(Unfinished::Announced));
        assert_eq!(guard.check(SHOWN, true), None);
        assert_eq!(guard.check("<|tool_call|>", true), None);
        guard.tool_ran();
        assert_eq!(guard.check(SHOWN, true), Some(Unfinished::Announced));
    }

    /// A reply the guard lets pass leaves its one request for a later reply.
    #[test]
    fn a_closing_reply_does_not_use_up_the_guard() {
        let mut guard = TurnEndGuard::default();
        assert_eq!(guard.check("I fixed the overlay.", true), None);
        assert_eq!(guard.check(SHOWN, true), Some(Unfinished::Announced));
    }

    fn filtered(shown: &str, deltas: &[&str]) -> String {
        let mut filter = RepeatFilter::new(shown);
        let mut out = String::new();
        for delta in deltas {
            out.extend(filter.push(delta));
        }
        out.extend(filter.finish());
        out
    }

    #[test]
    fn a_repeated_reply_is_not_shown_again() {
        assert_eq!(filtered(SHOWN, &[SHOWN]), "");
        assert_eq!(
            filtered(SHOWN, &["Let us aim", " and shoot the", " damaged enemy."]),
            ""
        );
        // Saying it again without the full stop, or with space around it, is the same reply.
        assert_eq!(
            filtered(SHOWN, &["  Let us aim and shoot the damaged enemy"]),
            ""
        );
        assert_eq!(filtered(SHOWN, &[SHOWN, "\n\n"]), "");
    }

    #[test]
    fn only_the_new_part_of_a_reply_is_shown_with_its_own_paragraph() {
        assert_eq!(
            filtered(SHOWN, &[SHOWN, "\n\nFiring ", "now."]),
            "\n\nFiring now."
        );
        assert_eq!(filtered(SHOWN, &["I hit", " it."]), "\n\nI hit it.");
        assert_eq!(filtered(SHOWN, &["Let us go."]), "\n\nLet us go.");
        // A reply that stops short of the whole line is new text.
        assert_eq!(filtered(SHOWN, &["Let us"]), "\n\nLet us");
    }

    #[test]
    fn a_retried_attempt_starts_over_without_a_second_break() {
        let mut filter = RepeatFilter::new(SHOWN);
        assert_eq!(filter.push("Hit.").as_deref(), Some("\n\nHit."));
        filter.restart();
        assert_eq!(filter.push("Hit.").as_deref(), Some("Hit."));
        filter.restart();
        assert_eq!(filter.push(SHOWN), None);
        assert_eq!(filter.finish(), None);
    }

    /// Text a model could write around the patterns the reading looks for.
    fn reply() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<String>(),
            proptest::collection::vec(
                prop_oneof![
                    Just("let me ".to_owned()),
                    Just("I'll ".to_owned()),
                    Just("moving ".to_owned()),
                    Just("you ".to_owned()),
                    Just(". ".to_owned()),
                    Just("?".to_owned()),
                    Just("\n".to_owned()),
                    Just("`".to_owned()),
                    Just("```".to_owned()),
                    Just("<|".to_owned()),
                    Just("|>".to_owned()),
                    Just("<\u{ff5c}".to_owned()),
                    Just("\u{ff5c}>".to_owned()),
                    Just("tool".to_owned()),
                    Just("{\"name\":\"a\",\"arguments\":{".to_owned()),
                    Just("}".to_owned()),
                    Just("[".to_owned()),
                    Just("\u{2026}".to_owned()),
                    Just("\u{e9}".to_owned()),
                    "[a-z]{1,8}",
                ],
                0..24,
            )
            .prop_map(|parts| parts.concat()),
        ]
    }

    /// Splits text on character boundaries, as a stream may.
    fn chunks(text: &str, cuts: &[usize]) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut points: Vec<usize> = cuts.iter().map(|cut| cut % (chars.len() + 1)).collect();
        points.extend([0, chars.len()]);
        points.sort_unstable();
        points.dedup();
        points
            .windows(2)
            .map(|pair| chars[pair[0]..pair[1]].iter().collect())
            .collect()
    }

    proptest! {
        /// Any model text reads without a panic, however it is shaped.
        #[test]
        fn any_reply_is_read_without_a_panic(text in reply(), worked in any::<bool>()) {
            let _ = unfinished_ending(&text, worked);
        }

        /// However many replies come between tool calls, at most one is sent back.
        #[test]
        fn the_guard_fires_at_most_once_per_stretch(
            stretches in proptest::collection::vec(proptest::collection::vec(reply(), 0..6), 0..6),
        ) {
            let mut guard = TurnEndGuard::default();
            for stretch in &stretches {
                let fired = stretch
                    .iter()
                    .filter(|text| guard.check(text, true).is_some())
                    .count();
                prop_assert!(fired <= 1);
                guard.tool_ran();
            }
        }

        /// A guard that always has a reason still fires only once, then again after a tool.
        #[test]
        fn a_stuck_model_gets_one_request_per_stretch(replies in 1usize..8, stretches in 1usize..5) {
            let mut guard = TurnEndGuard::default();
            for _ in 0..stretches {
                let fired = (0..replies).filter(|_| guard.check(SHOWN, true).is_some()).count();
                prop_assert_eq!(fired, 1);
                guard.tool_ran();
            }
        }

        /// However the stream splits it, a repeat shows only what follows it.
        #[test]
        fn a_repeat_is_dropped_however_it_is_split(
            rest in "[a-z ]{0,20}",
            cuts in proptest::collection::vec(0usize..200, 0..8),
        ) {
            let text = format!("{SHOWN}{rest}");
            let parts = chunks(&text, &cuts);
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            let out = filtered(SHOWN, &refs);
            prop_assert!(!out.contains(SHOWN));
            let expected = rest.trim_start();
            if expected.is_empty() {
                prop_assert_eq!(out, "");
            } else {
                prop_assert_eq!(out, format!("\n\n{expected}"));
            }
        }

        /// Other text is shown whole, once, whatever it holds.
        #[test]
        fn other_text_is_shown_whole(
            text in reply(),
            shown in reply(),
            cuts in proptest::collection::vec(0usize..200, 0..8),
        ) {
            let shown = format!("{}.", shown.trim());
            prop_assume!(!text.trim().is_empty());
            prop_assume!(!text.trim_start().starts_with(&shown));
            prop_assume!(
                text.trim().trim_end_matches(['.', '!', '\u{2026}'])
                    != shown.trim_end_matches(['.', '!', '\u{2026}'])
            );
            let parts = chunks(&text, &cuts);
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            prop_assert_eq!(filtered(&shown, &refs), format!("\n\n{}", text.trim_start()));
        }
    }
}
