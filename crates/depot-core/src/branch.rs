use crate::model::{Role, Task};

pub fn delivery_branch(task: &Task, taken: &[String]) -> String {
    let base = format!(
        "{}/{}",
        branch_prefix(task),
        slug(&task.title, task.id.as_str())
    );
    let mut candidate = base.clone();
    let mut suffix = 2;
    while taken.iter().any(|name| name == &candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

fn branch_prefix(task: &Task) -> &'static str {
    let text = format!("{} {}", task.title, task.intent).to_lowercase();
    let words: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if words
        .iter()
        .any(|word| matches!(*word, "refactor" | "restructure" | "rework"))
    {
        return "refactor";
    }
    if words.iter().any(|word| {
        matches!(
            *word,
            "fix" | "fixes" | "bug" | "bugs" | "repair" | "broken" | "regression" | "crash"
        )
    }) {
        return "fix";
    }
    if words.iter().any(|word| {
        matches!(
            *word,
            "docs" | "documentation" | "test" | "tests" | "chore" | "cleanup" | "config" | "deps"
        )
    }) {
        return "chore";
    }
    match task.role {
        Role::Build => "feat",
        Role::Fix => "fix",
        Role::Review => "refactor",
        Role::Plan => "chore",
    }
}

fn slug(title: &str, fallback: &str) -> String {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut slug = String::new();
    for word in &words {
        if slug.chars().count() + word.chars().count() + 1 > 40 && !slug.is_empty() {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(word);
    }
    if slug.is_empty() {
        return fallback
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join("-");
    }
    slug
}
