//! Examples double as test files. `cargo test` runs an example's tests only when
//! `Cargo.toml` declares it with `test = true`, so a new example without that
//! entry would silently drop out of CI.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[test]
fn every_example_runs_its_tests_under_cargo_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let mut tested = BTreeSet::new();
    let mut in_example = false;
    let mut name = None;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_example = line == "[[example]]";
            name = None;
        } else if in_example {
            if let Some(value) = line.strip_prefix("name = ") {
                name = Some(value.trim_matches('"').to_owned());
            } else if line == "test = true" {
                tested.extend(name.clone());
            }
        }
    }

    let examples: BTreeSet<String> = fs::read_dir(root.join("examples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        examples, tested,
        "declare every example in Cargo.toml as [[example]] with name first and test = true"
    );
}
