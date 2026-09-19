use depotd::{DEFAULT_ON_EVENTS, OnEventSettings, Settings};

#[test]
fn settings_without_on_event_keep_current_behaviour() {
    let settings = Settings::from_toml("").expect("empty settings parse");
    assert!(settings.on_event.is_none());
}

#[test]
fn without_an_event_list_only_the_blocking_events_fire() {
    let on_event = OnEventSettings {
        command: "curl -sf -d @- https://ntfy.sh/depot".to_owned(),
        events: None,
    };
    assert_eq!(
        on_event.command,
        "curl -sf -d @- https://ntfy.sh/depot".to_owned()
    );
    for event in DEFAULT_ON_EVENTS {
        assert!(on_event.includes(event), "{event} blocks by default");
    }
    assert!(!on_event.includes("landed"), "landing is opt-in");
}

#[test]
fn an_explicit_event_list_can_include_landed() {
    let settings =
        Settings::from_toml("[on_event]\ncommand = \"cat\"\nevents = [\"question\", \"landed\"]\n")
            .expect("settings parse");
    let on_event = settings.on_event.expect("an on_event command");
    assert_eq!(on_event.command, "cat");
    assert!(on_event.includes("question"));
    assert!(on_event.includes("landed"));
    assert!(!on_event.includes("failed"));
}
