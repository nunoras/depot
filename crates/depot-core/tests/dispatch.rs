use depot_core::{Confidence, DispatchRefusal, DispatchRule, ProfileId, Role, resolve_dispatch};
use std::collections::BTreeMap;

#[test]
fn dispatch_rules_apply_confidence_role_and_candidate_order() {
    let confidence = |value| Confidence::new(value).unwrap();
    let rules = vec![
        DispatchRule {
            when: "Changes code".into(),
            role: Role::Build,
            candidates: vec!["first".into(), "second".into()],
        },
        DispatchRule {
            when: "Reviews code".into(),
            role: Role::Review,
            candidates: vec![],
        },
        DispatchRule {
            when: "Plans work".into(),
            role: Role::Plan,
            candidates: vec![],
        },
    ];
    let profiles = BTreeMap::from([(Role::Review, ProfileId::from("reviewer"))]);
    for (choice, certainty, expected) in [
        (Some(0), 0.9, Ok((Role::Build, "first"))),
        (Some(1), 0.8, Ok((Role::Review, "reviewer"))),
        (Some(0), 0.79, Err(DispatchRefusal::BelowConfidenceFloor)),
        (None, 1.0, Err(DispatchRefusal::NoMatchingRule)),
        (Some(5), 1.0, Err(DispatchRefusal::NoMatchingRule)),
        (Some(2), 1.0, Err(DispatchRefusal::UnmappedRole(Role::Plan))),
    ] {
        let actual = resolve_dispatch(
            &rules,
            choice,
            confidence(certainty),
            confidence(0.8),
            &profiles,
        )
        .map(|result| (result.role, result.profile.to_string()));
        assert_eq!(
            actual,
            expected.map(|(role, profile)| (role, profile.to_owned()))
        );
    }
    for invalid in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(Confidence::new(invalid).is_none());
    }
}
