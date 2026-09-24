//! Ready-made `porta.toml` files for the coding agents people run under
//! porta, as data: each is what that agent was measured to need, with the
//! reasons written beside the keys. `porta init <name>` writes one.

/// Each recipe's name and text.
const RECIPES: [(&str, &str); 2] = [
    ("claude", include_str!("recipes/claude.toml")),
    ("codex", include_str!("recipes/codex.toml")),
];

/// The recipe called `name`, or an empty string when there is none.
pub fn wt_recipe(name: impl AsRef<str>) -> String {
    RECIPES.iter().find(|(recipe, _)| *recipe == name.as_ref()).map(|(_, text)| text.to_string()).unwrap_or_default()
}

/// The recipe names, space-separated, for help and errors.
pub fn wt_recipe_names() -> String {
    RECIPES.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recipe_is_a_native_porta_toml() {
        for (name, text) in RECIPES {
            let value: toml::Value = toml::from_str(text).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(value["runtime"]["type"].as_str(), Some("native"), "{name}");
            // porta reads `args` at the top level; under [runtime] it would be ignored
            assert!(value["runtime"].get("args").is_none(), "{name}");
            assert!(value["sandbox"]["mounts"].as_array().is_some_and(|mounts| mounts.iter().any(|m| m.as_str() == Some("."))), "{name}");
        }
        assert!(wt_recipe("nope").is_empty());
    }
}
