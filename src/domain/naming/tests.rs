use super::*;

#[test]
fn slugify_collapses_and_trims_separators() {
    for (name, slug) in [
        ("Living Lamp", "living-lamp"),
        ("  Hall   sensor #2 ", "hall-sensor-2"),
        ("PIR_1", "pir-1"),
        ("--x--", "x"),
        ("Лампа 2", "2"),
    ] {
        assert_eq!(slugify(name).unwrap(), slug, "{name:?}");
    }
}

#[test]
fn slugify_truncates_without_trailing_dash() {
    let slug = slugify(&format!("{} {}", "a".repeat(31), "b".repeat(10))).unwrap();
    assert_eq!(slug, "a".repeat(31));
}

#[test]
fn slugify_rejects_names_without_latin_characters() {
    for name in ["Лампа", "---", "💡"] {
        assert_eq!(slugify(name), Err(DomainError::EmptySlug));
    }
}

#[test]
fn valid_ids() {
    for id in ["lamp", "a", "living-lamp_2", &"a".repeat(32)] {
        assert!(is_valid_id(id), "{id:?}");
    }
    for id in ["", "Lamp", "my lamp", "lamp/1", "лампа", &"a".repeat(33)] {
        assert!(!is_valid_id(id), "{id:?}");
    }
}

#[test]
fn names_are_trimmed_and_checked() {
    assert_eq!(validate_name("  Lamp "), Ok("Lamp"));
    for name in ["", "   ", "tab\there", &"a".repeat(65)] {
        assert_eq!(
            validate_name(name),
            Err(DomainError::InvalidName),
            "{name:?}"
        );
    }
}
