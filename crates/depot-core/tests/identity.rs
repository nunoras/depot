use depot_core::remote_identity;

#[test]
fn the_three_forms_of_one_remote_are_one_identity() {
    for origin in [
        "git@github.com:O/R.git",
        "https://github.com/o/r/",
        "ssh://git@github.com/o/r",
    ] {
        assert_eq!(
            remote_identity(origin).as_deref(),
            Some("github.com/o/r"),
            "{origin}"
        );
    }
}

#[test]
fn a_port_and_a_user_are_dropped_and_the_host_is_lowercased() {
    assert_eq!(
        remote_identity("ssh://git@GitHub.com:8443/Owner/Repo.git").as_deref(),
        Some("github.com/owner/repo")
    );
}

#[test]
fn a_local_path_is_not_a_remote_identity() {
    assert_eq!(remote_identity("/work/origin.git"), None);
    assert_eq!(remote_identity("./origin.git"), None);
    assert_eq!(remote_identity(r"C:\work\origin.git"), None);
    assert_eq!(remote_identity("file:///work/origin.git"), None);
    assert_eq!(remote_identity(""), None);
}
