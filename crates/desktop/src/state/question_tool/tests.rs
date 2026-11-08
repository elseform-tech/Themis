use super::*;
use proptest::prelude::*;

fn choice(answer: &str) -> Value {
    json!({"answer": answer, "does": format!("do {answer}"), "advantages": "good", "disadvantages": "bad", "consequences": "easy to undo"})
}

#[test]
fn arguments_are_read_leniently() {
    let asked = parse(
        &json!({"question": "Tabs?", "choices": [choice("Yes"), choice("No")], "recommended": "Yes", "topic": "indent"}),
    )
    .unwrap();
    assert_eq!(asked.question, "Tabs?");
    assert_eq!(asked.choices, ["Yes", "No"]);
    assert_eq!(asked.details[0].1.does, "do Yes");
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

/// A choice without what it does, its advantages, disadvantages and consequences is refused,
/// naming what is missing; a bare string misses them all.
#[test]
fn a_choice_missing_details_is_refused_by_name() {
    let refused = parse(
        &json!({"question": "Tabs?", "choices": [choice("Tabs"), {"answer": "Spaces", "does": "re-indent", "advantages": "  "}]}),
    )
    .unwrap_err();
    assert!(refused.contains("choice 2 (\"Spaces\")"), "{refused}");
    // `does` was given: only the rest is named.
    assert!(
        refused.contains("is missing `advantages`, `disadvantages`, `consequences`."),
        "{refused}"
    );
    let bare =
        parse(&json!({"question": "Tabs?", "choices": ["Tabs", choice("Spaces")]})).unwrap_err();
    assert!(
        bare.contains(
            "choice 1 (\"Tabs\") is missing `does`, `advantages`, `disadvantages`, `consequences`"
        ),
        "{bare}"
    );
    let nameless =
        parse(&json!({"question": "Q", "choices": [{"does": "x"}, choice("B")]})).unwrap_err();
    assert!(
        nameless.contains("choice 1 is missing `answer`"),
        "{nameless}"
    );
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
            Just("does"),
            Just("advantages"),
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
            Just("does".to_owned()),
            Just("advantages".to_owned()),
            Just("disadvantages".to_owned()),
            Just("consequences".to_owned()),
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
    /// and every accepted choice carries all four details.
    #[test]
    fn untrusted_arguments_never_panic_and_accepted_choices_are_complete(args in arbitrary_json()) {
        if let Ok(pages) = pages(&args) {
            prop_assert!((1..=MAX_PAGES).contains(&pages.len()));
            for page in pages {
                if let Ok(asked) = parse(page) {
                    prop_assert_eq!(asked.choices.len(), asked.details.len());
                    for (answer, details) in &asked.details {
                        prop_assert!(!answer.trim().is_empty());
                        for field in [&details.does, &details.advantages, &details.disadvantages, &details.consequences] {
                            prop_assert!(!field.trim().is_empty());
                        }
                    }
                    if asked.blocking {
                        prop_assert!(asked.assumption.as_deref().is_none_or(|a| a.trim().is_empty()));
                    }
                }
            }
        }
    }
}
