//! Detects messages that carry no task (greetings, thanks, farewells) and extracts the
//! content words of a request for lexical recall. Input comes from the UI and is untrusted:
//! every function is total, bounded and allocation-light.
//!
//! A message is social only when every token is a social word or a neutral filler and at least
//! one token is a social word, so "hi, fix the auth bug", "thanks - now rename X", a bare
//! "perfect" or "great, continue" are all task messages. Spaced scripts match whole tokens;
//! unspaced scripts (Chinese, Japanese, Korean, Thai) match whole phrases at token
//! boundaries and never strip single characters from inside a content word.

use std::sync::LazyLock;

/// Longest request prefix examined, in characters.
const MAX_CHARS: usize = 2_000;
/// Most content terms returned.
pub const MAX_TERMS: usize = 12;

const STOP_WORDS: &[&str] = &[
    "what",
    "did",
    "discuss",
    "discussed",
    "about",
    "remember",
    "recall",
    "please",
    "continue",
    "that",
    "this",
    "then",
    "with",
    "have",
    "were",
    "earlier",
    "task",
    "topic",
    "topics",
    "the",
    "and",
    "from",
    "could",
    "would",
    "should",
    "again",
];

/// Words that make a message social when nothing else is present.
const SOCIAL_CORE: &[&str] = &[
    // English
    "hi",
    "hii",
    "hello",
    "hey",
    "heya",
    "hiya",
    "yo",
    "sup",
    "howdy",
    "greetings",
    "thanks",
    "thank",
    "thx",
    "ty",
    "tysm",
    "thankyou",
    "cheers",
    "bye",
    "goodbye",
    "cya",
    "morning",
    "afternoon",
    "evening",
    "night",
    "gm",
    "gn",
    // Spanish, Portuguese, Italian, French, German, Dutch, Scandinavian, Polish, Turkish
    "hola",
    "gracias",
    "adios",
    "buenas",
    "dias",
    "tardes",
    "noches",
    "ola",
    "obrigado",
    "obrigada",
    "tchau",
    "ciao",
    "grazie",
    "buongiorno",
    "salve",
    "bonjour",
    "salut",
    "merci",
    "bonsoir",
    "hallo",
    "danke",
    "tschuss",
    "tschüss",
    "dank",
    "dankjewel",
    "hej",
    "tack",
    "tak",
    "czesc",
    "cześć",
    "dziekuje",
    "dziękuję",
    "merhaba",
    "tesekkurler",
    "teşekkürler",
    "sagol",
    "sağol",
    "selam",
    "halo",
    "makasih",
    "terima",
    "kasih",
    // Russian, Ukrainian
    "привет",
    "здравствуйте",
    "спасибо",
    "пока",
    "дякую",
    "дякуємо",
    "привіт",
    // Arabic, Hebrew, Hindi
    "مرحبا",
    "شكرا",
    "اهلا",
    "أهلا",
    "سلام",
    "שלום",
    "תודה",
    "नमस्ते",
    "धन्यवाद",
    "शुक्रिया",
];

/// Neutral words allowed around a social word ("thank you so much", "good morning").
const FILLER: &[&str] = &[
    "you",
    "u",
    "so",
    "much",
    "very",
    "a",
    "lot",
    "there",
    "all",
    "good",
    "again",
    "for",
    "your",
    "help",
    "everyone",
    "team",
    "friend",
    "buen",
    "muchas",
    "muito",
    "добрый",
    "день",
    "вечер",
    "утро",
    "дуже",
];

/// Whole-message social phrases made of ordinary words.
const SOCIAL_PHRASES: &[&str] = &[
    "how are you",
    "how are you doing",
    "how are u",
    "hows it going",
    "how is it going",
    "whats up",
    "how do you do",
];

/// Unspaced-script phrases (matched whole, at token boundaries).
const UNSPACED_PHRASES: &[&str] = &[
    "你好",
    "您好",
    "哈喽",
    "嗨",
    "谢谢",
    "谢谢你",
    "谢谢您",
    "感谢",
    "多谢",
    "再见",
    "拜拜",
    "早上好",
    "晚上好",
    "早安",
    "晚安",
    "午安",
    "謝謝",
    "感謝",
    "再見",
    "こんにちは",
    "こんばんは",
    "おはよう",
    "ありがとう",
    "ありがとうございます",
    "さようなら",
    "どうも",
    "안녕",
    "안녕하세요",
    "감사합니다",
    "고마워",
    "고마워요",
    "고맙습니다",
    "สวัสดี",
    "ขอบคุณ",
];

/// Particles that may trail an unspaced social phrase ("你好吗", "谢谢啦").
const UNSPACED_PARTICLES: &[&str] = &["啦", "啊", "呀", "吗", "呢", "吧", "哈", "喔", "哦"];

static PHRASES_BY_LENGTH: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut phrases = UNSPACED_PHRASES.to_vec();
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.chars().count()));
    phrases
});

fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0x0E00..=0x0E7F
        | 0xF900..=0xFAFF)
}

/// Combining marks and Indic vowel signs belong to the word they modify.
fn is_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2
        | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670
        | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
        | 0x0900..=0x0903 | 0x093A..=0x093C | 0x093E..=0x094F | 0x0951..=0x0957
        | 0x0962..=0x0963 | 0x0981..=0x0983 | 0x09BC..=0x09CD | 0x0A01..=0x0A03
        | 0x0A3C..=0x0A51 | 0x0A81..=0x0A83 | 0x0ABC..=0x0ACD | 0x0B01..=0x0B03
        | 0x0B3C..=0x0B57 | 0x0B82 | 0x0BBE..=0x0BCD | 0x0C00..=0x0C04 | 0x0C3E..=0x0C56
        | 0x0C81..=0x0C83 | 0x0CBC..=0x0CD6 | 0x0D00..=0x0D03 | 0x0D3B..=0x0D3C
        | 0x0D3E..=0x0D4D | 0x0D81..=0x0D83 | 0x0DCA..=0x0DDF | 0x0E31 | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECD | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0x3099..=0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c)
}

/// Lowercase tokens; apostrophes join ("what's" is "whats"), marks stay in their word.
fn tokens(text: &str) -> Vec<String> {
    let lowered = text
        .chars()
        .take(MAX_CHARS)
        .filter(|c| !matches!(c, '\'' | '\u{2019}'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    lowered
        .split(|c: char| !is_word_char(c))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Matching form: drops the dot Turkish `İ` leaves behind and Arabic/Hebrew vowel points.
fn match_form(token: &str) -> String {
    token
        .chars()
        .filter(|c| !matches!(*c as u32, 0x0307 | 0x0591..=0x05BD | 0x064B..=0x065F | 0x0670))
        .collect()
}

/// Splits an unspaced token into leading social phrases (each with trailing particles).
/// Returns the unconsumed remainder.
fn strip_leading_phrases(token: &str) -> &str {
    let mut rest = token;
    'outer: loop {
        for phrase in PHRASES_BY_LENGTH.iter() {
            if let Some(after) = rest.strip_prefix(phrase) {
                rest = after;
                while let Some(after) = UNSPACED_PARTICLES
                    .iter()
                    .find_map(|particle| rest.strip_prefix(particle))
                {
                    rest = after;
                }
                continue 'outer;
            }
        }
        return rest;
    }
}

fn strip_trailing_phrases(token: &str) -> &str {
    let mut rest = token;
    'outer: loop {
        for phrase in PHRASES_BY_LENGTH.iter() {
            if let Some(before) = rest.strip_suffix(phrase) {
                rest = before;
                continue 'outer;
            }
        }
        return rest;
    }
}

/// True for a message that is only a greeting, thanks or farewell, with at least one social
/// word. "continue", acknowledgements ("ok", "perfect", "great") and anything with task
/// content are not social: they refer back to the conversation.
pub fn is_social_request(request: &str) -> bool {
    let words = tokens(request);
    if words.is_empty() {
        return false;
    }
    let forms = words
        .iter()
        .map(|word| match_form(word))
        .collect::<Vec<_>>();
    if forms.join(" ").trim().is_empty() {
        return false;
    }
    // "how are you" and "hey, how's it going" are social as a whole phrase.
    let after_greeting = forms
        .iter()
        .skip_while(|form| SOCIAL_CORE.contains(&form.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if SOCIAL_PHRASES.contains(&after_greeting.as_str()) {
        return true;
    }
    let mut social = false;
    for form in &forms {
        if form.chars().any(is_unspaced_script) {
            if !strip_leading_phrases(form).is_empty() {
                return false;
            }
            social = true;
        } else if SOCIAL_CORE.contains(&form.as_str()) {
            social = true;
        } else if !FILLER.contains(&form.as_str()) {
            return false;
        }
    }
    social
}

/// Content words for lexical recall. Spaced scripts need three characters and a non-stop,
/// non-social word; unspaced scripts contribute their runs minus leading and trailing social
/// phrases (two characters or more). At most [`MAX_TERMS`] unique terms, sorted.
pub fn content_terms(lower: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for word in tokens(lower) {
        let form = match_form(&word);
        if form.chars().any(is_unspaced_script) {
            let core = strip_trailing_phrases(strip_leading_phrases(&form));
            if core.chars().count() >= 2 {
                terms.push(core.to_owned());
            }
        } else if form.chars().count() >= 3
            && !STOP_WORDS.contains(&form.as_str())
            && !SOCIAL_CORE.contains(&form.as_str())
            && !FILLER.contains(&form.as_str())
        {
            terms.push(form);
        }
    }
    terms.sort();
    terms.dedup();
    terms.truncate(MAX_TERMS);
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn greetings_in_many_languages_are_social() {
        for text in [
            "hi",
            "Thanks!",
            "hello there",
            "thank you so much",
            "good morning",
            "how are you?",
            "hey, how's it going?",
            "What's up",
            "你好",
            "谢谢你！",
            "你好吗",
            "谢谢啦",
            "こんにちは",
            "ありがとうございます",
            "안녕하세요",
            "Спасибо!",
            "gracias",
            "नमस्ते",
            "धन्यवाद!",
            "Merhaba!",
            "مرحبا",
            "مَرْحَبًا",
            "สวัสดี",
            "ขอบคุณ",
            "👋 hi",
            "hi 😀",
        ] {
            assert!(is_social_request(text), "{text:?}");
        }
    }

    #[test]
    fn task_messages_are_not_social() {
        for text in [
            "hi, fix the auth bug",
            "thanks - now also rename X",
            "perfect",
            "great",
            "great, continue",
            "good",
            "you",
            "there",
            "continue",
            "ok",
            "do it",
            "run the tests",
            "你好，帮我看看登录问题",
            "你的代码",
            "酒吧台",
            "谢谢你的代码",
            "",
            "   ",
            "👋",
            "नमस्ते दुनिया कोड ठीक करो",
            "İstanbul trip budget",
            "hello world",
        ] {
            assert!(!is_social_request(text), "{text:?}");
        }
    }

    #[test]
    fn combining_marks_stay_inside_words() {
        assert_eq!(content_terms("नमस्ते"), Vec::<String>::new()); // social word, not garbled
        assert_eq!(content_terms("कोड त्रुटि ठीक"), vec!["कोड", "ठीक", "त्रुटि"]);
        let terms = content_terms(&"İstanbul bütçe".to_lowercase());
        assert!(terms.iter().any(|t| t == "istanbul"), "{terms:?}");
        assert_eq!(content_terms("ภาษาไทย"), vec!["ภาษาไทย"]);
        assert_eq!(content_terms("مشروع الترجمة"), vec!["الترجمة", "مشروع"]);
    }

    #[test]
    fn cjk_social_phrases_only_strip_at_boundaries() {
        assert_eq!(content_terms("你的代码"), vec!["你的代码"]);
        assert_eq!(content_terms("酒吧台"), vec!["酒吧台"]);
        assert_eq!(content_terms("你好请回忆"), vec!["请回忆"]);
        assert_eq!(content_terms("请回忆谢谢"), vec!["请回忆"]);
        assert!(content_terms("你好吗").is_empty());
    }

    #[test]
    fn terms_are_bounded() {
        let long = (0..5_000)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(content_terms(&long).len() <= MAX_TERMS);
        assert!(!is_social_request(&format!("hi {long}")));
    }

    proptest! {
        #[test]
        fn never_panics_and_stays_bounded(text in any::<String>()) {
            let _ = is_social_request(&text);
            let terms = content_terms(&text);
            prop_assert!(terms.len() <= MAX_TERMS);
            prop_assert!(terms.iter().all(|t| t.chars().count() <= MAX_CHARS));
            prop_assert!(terms.windows(2).all(|w| w[0] < w[1]));
        }

        #[test]
        fn social_only_input_is_social_and_idempotent(
            picks in proptest::collection::vec(0usize..SOCIAL_CORE.len(), 1..6),
            sep in "[ ,.!?-]{1,3}",
        ) {
            let text = picks.iter().map(|i| SOCIAL_CORE[*i]).collect::<Vec<_>>().join(&sep);
            prop_assert!(is_social_request(&text), "{}", text);
            let doubled = format!("{text}{sep}{text}");
            prop_assert!(is_social_request(&doubled), "{}", doubled);
            prop_assert!(content_terms(&text).is_empty(), "{}", text);
        }

        #[test]
        fn any_non_social_ascii_word_makes_it_a_task(
            before in ".{0,200}",
            after in ".{0,200}",
            word in "qz[a-z]{2,8}",
        ) {
            let text = format!("{before} | {word} | {after}");
            prop_assert!(!is_social_request(&text), "{}", text);
        }
    }
}
