use super::*;

#[test]
fn serializes_as_a_plain_json_value() {
    assert_eq!(serde_json::to_string(&State::Bool(true)).unwrap(), "true");
    assert_eq!(serde_json::to_string(&State::Int(-5)).unwrap(), "-5");
}

#[test]
fn deserializes_booleans_and_integers() {
    for (json, expected) in [
        ("true", State::Bool(true)),
        ("false", State::Bool(false)),
        ("0", State::Int(0)),
        ("-40", State::Int(-40)),
        ("2147483647", State::Int(i32::MAX)),
        ("-2147483648", State::Int(i32::MIN)),
    ] {
        assert_eq!(
            serde_json::from_str::<State>(json).unwrap(),
            expected,
            "{json}"
        );
    }
}

#[test]
fn rejects_everything_else() {
    for json in ["1.5", r#""on""#, "null", "[]", "2147483648", "-2147483649"] {
        assert!(serde_json::from_str::<State>(json).is_err(), "{json}");
    }
}

#[test]
fn errors_say_what_is_expected() {
    for json in ["1.5", r#""on""#, "null", "3000000000", "-2147483649"] {
        let message = serde_json::from_str::<State>(json).unwrap_err().to_string();
        assert!(
            message.contains("true, false or a whole number between"),
            "{json}: {message}"
        );
        assert!(!message.contains("untagged"), "{json}: {message}");
    }
}

#[test]
fn display_is_human_readable() {
    assert_eq!(State::Bool(true).to_string(), "on");
    assert_eq!(State::Bool(false).to_string(), "off");
    assert_eq!(State::Int(-3).to_string(), "-3");
}

#[test]
fn conversions() {
    assert_eq!(State::from(true), State::Bool(true));
    assert_eq!(State::from(7), State::Int(7));
}
