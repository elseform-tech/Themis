use super::*;
use proptest::prelude::*;

fn choice(answer: &str) -> Value {
    json!({"answer": answer, "hint": format!(
        "Does {answer} across the project in one pass, which is quick and keeps the code consistent, \
         but it touches many files at once and makes the history noisy for a while; it is easy to undo with one plain revert."
    )})
}

#[test]
fn arguments_are_read_leniently() {
    let asked = parse(
        &json!({"question": "Tabs?", "choices": [choice("Yes"), choice("No")], "recommended": "Yes", "topic": "indent"}),
    )
    .unwrap();
    assert_eq!(asked.question, "Tabs?");
    assert_eq!(asked.choices, ["Yes", "No"]);
    assert_eq!(asked.reasons[0].1, choice("Yes")["hint"]);
    assert_eq!(asked.assumption.as_deref(), Some("Yes"));
    assert!(!asked.blocking);
    assert_eq!(asked.topic.as_deref(), Some("indent"));
    assert!(
        parse(&json!({"question": "Key?", "blocking": true}))
            .unwrap()
            .blocking
    );
    // With a recommendation it does not block, whatever the model asked for.
    let recommended = parse(
        &json!({"question": "Which analytics?", "choices": [choice("Product"), choice("Web")], "recommended": "Product", "blocking": true}),
    )
    .unwrap();
    assert!(!recommended.blocking);
    // The older name still reads.
    assert_eq!(
        parse(&json!({"question": "Q", "assumption": "A"}))
            .unwrap()
            .assumption
            .as_deref(),
        Some("A")
    );
}

/// A hint too short to say what happens, what it costs and what follows is refused, naming the
/// choice, its length and the floor; one at the floor is taken.
#[test]
fn a_thin_hint_is_refused_with_its_length() {
    let thin = "Largest ecosystem, huge hiring pool. Good default for most SaaS.";
    let refused = parse(&json!({"question": "Stack?", "choices": [choice("Go"), {"answer": "TypeScript", "hint": thin}]}))
        .unwrap_err();
    assert!(refused.contains("choice 2 (\"TypeScript\")"), "{refused}");
    assert!(
        refused.contains(&format!("{} characters", thin.chars().count())),
        "{refused}"
    );
    assert!(refused.contains(&MIN_HINT_CHARS.to_string()), "{refused}");
    let at_floor = "é".repeat(MIN_HINT_CHARS);
    assert!(
        parse(&json!({"question": "Stack?", "choices": [{"answer": "Go", "hint": at_floor}]}))
            .is_ok()
    );
}

/// A choice without its hint is refused, naming what is missing; a bare string has none.
#[test]
fn a_choice_without_a_hint_is_refused_by_name() {
    let refused = parse(
        &json!({"question": "Tabs?", "choices": [choice("Tabs"), {"answer": "Spaces", "hint": "  "}]}),
    )
    .unwrap_err();
    assert!(
        refused.contains("choice 2 (\"Spaces\") needs `hint`."),
        "{refused}"
    );
    let bare =
        parse(&json!({"question": "Tabs?", "choices": ["Tabs", choice("Spaces")]})).unwrap_err();
    assert!(bare.contains("choice 1 (\"Tabs\") needs `hint`"), "{bare}");
    let nameless =
        parse(&json!({"question": "Q", "choices": [{"hint": "x"}, choice("B")]})).unwrap_err();
    assert!(nameless.contains("choice 1 needs `answer`"), "{nameless}");
    // A question without choices (a free answer) needs none of it.
    assert!(parse(&json!({"question": "Name?", "recommended": "Pebble"})).is_ok());
}

#[test]
fn up_to_four_questions_ride_in_one_call() {
    let one = json!({"question": "Tabs?"});
    assert_eq!(pages(&one).unwrap().len(), 1);
    let three = json!({"questions": [{"question": "A?"}, {"question": "B?"}, {"question": "C?"}]});
    assert_eq!(pages(&three).unwrap().len(), 3);
    for refused in [
        json!({"questions": []}),
        json!({"questions": [{"question": "A?"}, {"question": "B?"}, {"question": "C?"}, {"question": "D?"}, {"question": "E?"}]}),
        json!({"question": "A?", "questions": [{"question": "B?"}]}),
        json!({"questions": "A?"}),
    ] {
        let note = pages(&refused).unwrap_err();
        assert!(note.contains("1 to 4 questions"), "{note}");
    }
}

fn arbitrary_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,40}".prop_map(Value::String),
        prop_oneof![
            Just("question"),
            Just("choices"),
            Just("answer"),
            Just("hint"),
            Just("questions"),
            Just("recommended"),
            Just("blocking")
        ]
        .prop_map(|key| json!(key)),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        let keys = prop_oneof![
            Just("question".to_owned()),
            Just("questions".to_owned()),
            Just("choices".to_owned()),
            Just("answer".to_owned()),
            Just("hint".to_owned()),
            Just("recommended".to_owned()),
            Just("blocking".to_owned()),
            Just("why".to_owned()),
            Just("topic".to_owned()),
            ".{0,8}",
        ];
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(keys, inner, 0..8)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Whatever the model sends, reading it never panics; what is accepted has 1 to 4 pages,
    /// and every accepted choice carries a hint.
    #[test]
    fn untrusted_arguments_never_panic_and_accepted_choices_are_complete(args in arbitrary_json()) {
        if let Ok(pages) = pages(&args) {
            prop_assert!((1..=MAX_PAGES).contains(&pages.len()));
            for page in pages {
                if let Ok(asked) = parse(page) {
                    prop_assert_eq!(asked.choices.len(), asked.reasons.len());
                    for (answer, hint) in &asked.reasons {
                        prop_assert!(!answer.trim().is_empty());
                        prop_assert!(hint.trim().chars().count() >= MIN_HINT_CHARS);
                    }
                    if asked.blocking {
                        prop_assert!(asked.assumption.as_deref().is_none_or(|a| a.trim().is_empty()));
                    }
                }
            }
        }
    }
}
