//! The roles and recipes that come with Genatrix.
//!
//! Design: `docs/design/13-executors.md`, "角色不是进程": product, UI,
//! frontend, backend, test and operations. They are files like any the user
//! adds, read the same way and identified the same way, by hash. The
//! operations role writes a change plan and runs nothing (ruling 6).

use crate::recipe::Recipe;
use crate::role::Role;

/// The built-in role files, by name.
pub const ROLE_FILES: &[(&str, &str)] = &[
    ("product", include_str!("../builtin/product.toml")),
    ("ui", include_str!("../builtin/ui.toml")),
    ("frontend", include_str!("../builtin/frontend.toml")),
    ("backend", include_str!("../builtin/backend.toml")),
    ("test", include_str!("../builtin/test.toml")),
    ("ops", include_str!("../builtin/ops.toml")),
];

/// The built-in recipe files, by name.
pub const RECIPE_FILES: &[(&str, &str)] = &[
    (
        "implement-and-review",
        include_str!("../builtin/implement-and-review.toml"),
    ),
    (
        "spec-then-build",
        include_str!("../builtin/spec-then-build.toml"),
    ),
    ("implement", include_str!("../builtin/implement.toml")),
    ("review-only", include_str!("../builtin/review-only.toml")),
    ("change-plan", include_str!("../builtin/change-plan.toml")),
];

/// The built-in workspace files, by name (design 14): `personal`, with no
/// accounts, where a project goes unless it says otherwise.
pub const WORKSPACE_FILES: &[(&str, &str)] = &[(
    crate::workspace::PERSONAL,
    "name = \"personal\"\ntitle = \"Personal\"\n",
)];

/// The built-in roles.
///
/// # Panics
///
/// If a built-in file does not parse, which a test rules out.
#[must_use]
pub fn roles() -> Vec<Role> {
    ROLE_FILES
        .iter()
        .map(|(_, text)| Role::parse(text).expect("built-in role parses"))
        .collect()
}

/// The built-in recipes.
///
/// # Panics
///
/// If a built-in file does not parse, which a test rules out.
#[must_use]
pub fn recipes() -> Vec<Recipe> {
    RECIPE_FILES
        .iter()
        .map(|(_, text)| Recipe::parse(text).expect("built-in recipe parses"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::May;

    #[test]
    fn every_builtin_reads_and_is_named_as_filed() {
        for (name, role) in ROLE_FILES.iter().map(|(n, _)| *n).zip(roles()) {
            assert_eq!(name, role.name);
        }
        for (name, recipe) in RECIPE_FILES.iter().map(|(n, _)| *n).zip(recipes()) {
            assert_eq!(name, recipe.name);
        }
        for (name, text) in WORKSPACE_FILES {
            let w = crate::workspace::Workspace::parse(text).unwrap();
            assert_eq!(*name, w.name);
            assert!(
                w.accounts.is_empty(),
                "the built-in workspace has no accounts"
            );
        }
        let names: Vec<String> = roles().into_iter().map(|r| r.name).collect();
        for recipe in recipes() {
            for step in &recipe.steps {
                assert!(names.contains(&step.role), "{}: {}", recipe.name, step.role);
            }
        }
    }

    /// Design 13, ruling 6: operations plans; it does not run anything.
    #[test]
    fn the_operations_role_runs_nothing() {
        let ops = roles().into_iter().find(|r| r.name == "ops").unwrap();
        assert!(!ops.may(May::Commands));
        assert!(!ops.may(May::Edit));
        assert!(!ops.may(May::Network));
    }

    #[test]
    fn reviewers_and_writers_of_documents_change_no_files() {
        for name in ["test", "product", "ui"] {
            let r = roles().into_iter().find(|r| r.name == name).unwrap();
            assert!(!r.may(May::Edit), "{name}");
        }
    }
}
