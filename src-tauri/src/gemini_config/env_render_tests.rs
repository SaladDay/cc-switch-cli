use super::{serialize_env_file, write_gemini_env_atomic};
use crate::test_support::TestEnvGuard;
use std::{collections::HashMap, fs};

// CLI d4945cd7. This oracle deliberately does not use the shared renderer.
fn previous_render(map: &HashMap<String, String>) -> String {
    let mut lines = Vec::new();
    let mut keys: Vec<_> = map.keys().collect();
    keys.sort();
    for key in keys {
        if let Some(value) = map.get(key) {
            lines.push(format!("{key}={value}"));
        }
    }
    lines.join("\n")
}

#[test]
fn gemini_env_render_matches_previous_literal_bytes() {
    let names = [
        "",
        "A",
        "a",
        "0_NAME",
        "变量",
        "bad=name",
        "bad\nname",
        "bad\0name",
    ];
    let values = [
        "",
        "plain",
        "a=b",
        "  literal  ",
        "'quoted'",
        "$TOKEN",
        "a\r\nb\0",
    ];
    assert_eq!(serialize_env_file(&HashMap::new()), "");
    for name in names {
        for value in values {
            let mut entries = HashMap::from([
                ("Z".to_owned(), "last".to_owned()),
                ("B".to_owned(), "middle".to_owned()),
            ]);
            entries.insert(name.to_owned(), value.to_owned());
            assert_eq!(serialize_env_file(&entries), previous_render(&entries));
        }
    }
    let large = HashMap::from([("LARGE".into(), "x".repeat(1024 * 1024 + 1))]);
    assert_eq!(serialize_env_file(&large), previous_render(&large));
}

#[test]
fn gemini_env_writer_keeps_previous_bytes_and_empty_file_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = super::get_gemini_env_path();
    for entries in [
        HashMap::from([
            ("GEMINI_API_KEY".into(), "fake-key".into()),
            ("变量".into(), "literal='x=y'\nextra".into()),
        ]),
        HashMap::new(),
    ] {
        write_gemini_env_atomic(&entries).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            previous_render(&entries).as_bytes()
        );
    }
}
