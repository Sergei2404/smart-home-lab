use super::*;
use crate::domain::DomainError;

#[test]
fn id_comes_from_the_name() {
    assert_eq!(
        ProjectId::from_name("My Dacha 2026").unwrap().as_str(),
        "my-dacha-2026"
    );
    assert_eq!(ProjectId::from_name("Дача"), Err(DomainError::EmptySlug));
}

#[test]
fn ids_cannot_escape_the_projects_folder_or_clash_with_the_active_marker() {
    for bad in [
        "../secret",
        "a/b",
        "a\\b",
        ".active",
        ".",
        "..",
        "x.json",
        "",
        "A B",
    ] {
        assert!(ProjectId::parse(bad).is_err(), "{bad:?} must be rejected");
    }
    assert!(ProjectId::parse("default").is_ok());
}

#[test]
fn the_default_project_has_a_valid_id() {
    let id = ProjectId::default_project();
    assert_eq!(ProjectId::parse(id.as_str()), Ok(id));
}

#[test]
fn serializes_as_a_plain_string() {
    let id = ProjectId::parse("home").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""home""#);
    assert!(serde_json::from_str::<ProjectId>(r#""../x""#).is_err());
}
