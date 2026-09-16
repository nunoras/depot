use depot_core::{ProfileId, Role};
use depotd::adapters::profiles::{ConfiguredProfiles, ProfileResolver, ProfileSpec, RoleEntry};

fn spec(profile: &str, model: &str) -> ProfileSpec {
    ProfileSpec {
        profile: ProfileId::new(profile),
        harness: "claude".to_owned(),
        model: model.to_owned(),
        effort: "high".to_owned(),
    }
}

#[test]
fn resolves_a_role_to_its_profile_and_its_fallbacks() {
    let profiles = ConfiguredProfiles::from_entries([
        RoleEntry {
            role: Role::Build,
            profiles: vec![
                spec("work", "glm-5.3"),
                spec("backup", "glm-5.3-flash"),
                spec("last-resort", "glm-4.5"),
            ],
        },
        RoleEntry {
            role: Role::Review,
            profiles: vec![spec("review-only", "gpt-5.5")],
        },
    ])
    .expect("the configured roles are complete");

    let build = profiles.resolve(Role::Build).expect("build resolves");
    assert_eq!(build.role, Role::Build);
    assert_eq!(build.primary.profile, ProfileId::new("work"));
    assert_eq!(build.primary.harness, "claude");
    assert_eq!(build.primary.model, "glm-5.3");
    assert_eq!(build.primary.effort, "high");
    assert_eq!(
        build
            .fallbacks
            .iter()
            .map(|spec| spec.profile.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["backup".to_owned(), "last-resort".to_owned()]
    );

    let review = profiles.resolve(Role::Review).expect("review resolves");
    assert_eq!(review.primary.model, "gpt-5.5");
    assert!(review.fallbacks.is_empty());
    assert_eq!(profiles.roles(), vec![Role::Build, Role::Review]);
}

#[test]
fn refuses_an_unmapped_role_instead_of_guessing() {
    let profiles = ConfiguredProfiles::from_entries([RoleEntry {
        role: Role::Build,
        profiles: vec![spec("work", "glm-5.3")],
    }])
    .expect("the configured role is complete");

    let error = profiles
        .resolve(Role::Review)
        .expect_err("an unmapped role is refused");
    let message = error.to_string();
    assert!(
        message.contains("no profile is configured for the role review"),
        "{message}"
    );
    assert!(message.contains("configured roles: build"), "{message}");
    assert!(message.contains("not a role depot guesses"), "{message}");
}

#[test]
fn refuses_a_role_configured_with_no_profile() {
    let error = ConfiguredProfiles::from_entries([RoleEntry {
        role: Role::Fix,
        profiles: Vec::new(),
    }])
    .expect_err("a role without profiles is refused");
    assert!(
        error.to_string().contains("configured with no profile"),
        "{error}"
    );
}

#[test]
fn refuses_an_incomplete_profile() {
    let error = ConfiguredProfiles::from_entries([RoleEntry {
        role: Role::Plan,
        profiles: vec![ProfileSpec {
            profile: ProfileId::new("half"),
            harness: "claude".to_owned(),
            model: String::new(),
            effort: "high".to_owned(),
        }],
    }])
    .expect_err("a profile without a model is refused");
    let message = error.to_string();
    assert!(message.contains("half"), "{message}");
    assert!(message.contains("declares no model"), "{message}");
}

#[test]
fn refuses_a_role_configured_twice() {
    let error = ConfiguredProfiles::from_entries([
        RoleEntry {
            role: Role::Build,
            profiles: vec![spec("work", "glm-5.3")],
        },
        RoleEntry {
            role: Role::Build,
            profiles: vec![spec("other", "glm-4.5")],
        },
    ])
    .expect_err("a role configured twice is refused");
    assert!(
        error.to_string().contains("configured more than once"),
        "{error}"
    );
}
