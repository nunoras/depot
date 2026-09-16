#[test]
fn depot_core_declares_no_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("[dependencies]"),
        "depot-core/Cargo.toml must declare its empty [dependencies] table"
    );
    let declared: Vec<&str> = manifest
        .lines()
        .skip_while(|line| line.trim() != "[dependencies]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        declared.is_empty(),
        "depot-core is pure: it takes facts in and returns state and actions out, so it must not depend on anything, found {declared:?}"
    );
}
