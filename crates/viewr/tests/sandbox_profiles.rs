//! Repository-level checks for the three network-denied packaging profiles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use roxmltree::{Document, ParsingOptions};

const UAP10_NAMESPACE: &str = "http://schemas.microsoft.com/appx/manifest/uap/windows10/10";

fn repository_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn read_repository_file(relative: &str) -> String {
    std::fs::read_to_string(repository_path(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn yaml_scalar<'a>(source: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    source.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .map(|value| value.trim_matches(['\'', '"']))
    })
}

fn yaml_list<'a>(source: &'a str, key: &str) -> Vec<&'a str> {
    let header = format!("{key}:");
    let mut inside = false;
    let mut values = Vec::new();

    for line in source.lines() {
        if line == header {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if !line.is_empty() && !line.starts_with(char::is_whitespace) {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("- ") {
            values.push(value.trim_matches(['\'', '"']));
        }
    }
    values
}

fn true_entitlements(source: &str) -> BTreeSet<String> {
    let document = Document::parse_with_options(
        source,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .expect("parse entitlement plist");
    let dictionary = document
        .descendants()
        .find(|node| node.has_tag_name("dict"))
        .expect("entitlement plist has a dictionary");
    let elements: Vec<_> = dictionary
        .children()
        .filter(roxmltree::Node::is_element)
        .collect();
    assert_eq!(
        elements.len() % 2,
        0,
        "entitlement dictionary contains complete key/value pairs"
    );

    let mut entitlements = BTreeSet::new();
    for pair in elements.chunks_exact(2) {
        assert!(pair[0].has_tag_name("key"), "expected entitlement key");
        assert!(
            pair[1].has_tag_name("true"),
            "every checked entitlement must be boolean true"
        );
        let key = pair[0].text().expect("entitlement key has text").to_owned();
        assert!(
            entitlements.insert(key.clone()),
            "duplicate entitlement key: {key}"
        );
    }
    entitlements
}

#[test]
fn flatpak_profile_has_only_reviewed_runtime_grants() {
    let source = read_repository_file("packaging/flatpak/com.github.blisspixel.viewr.yml");
    assert_eq!(
        source
            .lines()
            .filter(|line| *line == "finish-args:")
            .count(),
        1,
        "manifest has exactly one effective Flatpak grant list"
    );
    assert_eq!(
        yaml_scalar(&source, "app-id"),
        Some("com.github.blisspixel.viewr")
    );
    assert_eq!(yaml_scalar(&source, "runtime-version"), Some("25.08"));

    let grants = yaml_list(&source, "finish-args");
    let actual: BTreeSet<_> = grants.iter().copied().collect();
    assert_eq!(
        grants.len(),
        actual.len(),
        "duplicate Flatpak runtime grant"
    );
    let expected = BTreeSet::from([
        "--device=dri",
        "--share=ipc",
        "--socket=fallback-x11",
        "--socket=wayland",
    ]);
    assert_eq!(
        actual, expected,
        "every Flatpak runtime grant requires an explicit test review"
    );

    for build_invariant in [
        "append-path: /usr/lib/sdk/rust-stable/bin",
        "CARGO_HOME: /run/build/viewr/cargo",
        "cargo --offline build --locked --release --workspace",
        "- cargo-sources.json",
    ] {
        assert!(
            source.contains(build_invariant),
            "Flatpak build invariant missing: {build_invariant}"
        );
    }
    let cargo_sources = read_repository_file("packaging/flatpak/cargo-sources.json");
    assert!(
        cargo_sources.contains("https://static.crates.io/crates/"),
        "Flatpak Cargo sources are populated"
    );
    assert!(
        cargo_sources.contains("[source.vendored-sources]"),
        "Flatpak Cargo sources configure offline vendoring"
    );
}

#[test]
fn macos_profiles_have_exact_minimal_entitlements() {
    let main = read_repository_file("packaging/macos/viewr.entitlements");
    assert_eq!(
        true_entitlements(&main),
        BTreeSet::from([
            "com.apple.security.app-sandbox".to_owned(),
            "com.apple.security.files.user-selected.read-write".to_owned(),
        ])
    );

    let worker = read_repository_file("packaging/macos/viewr-decode.entitlements");
    assert_eq!(
        true_entitlements(&worker),
        BTreeSet::from([
            "com.apple.security.app-sandbox".to_owned(),
            "com.apple.security.inherit".to_owned(),
        ])
    );
}

#[test]
fn windows_profile_is_appcontainer_with_no_capabilities() {
    let source = read_repository_file("packaging/windows/AppxManifest.xml");
    let document = Document::parse(&source).expect("parse AppxManifest.xml");
    let applications: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("Application"))
        .collect();
    assert_eq!(
        applications.len(),
        1,
        "manifest has exactly one application"
    );
    let application = applications[0];

    assert_eq!(application.attribute("Executable"), Some("viewr.exe"));
    assert_eq!(
        application.attribute("EntryPoint"),
        Some("Windows.PartialTrustApplication")
    );
    assert_eq!(
        application.attribute((UAP10_NAMESPACE, "RuntimeBehavior")),
        Some("packagedClassicApp")
    );
    assert_eq!(
        application.attribute((UAP10_NAMESPACE, "TrustLevel")),
        Some("appContainer")
    );

    let capability_nodes: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("Capabilities"))
        .collect();
    assert_eq!(
        capability_nodes.len(),
        1,
        "manifest has exactly one explicit Capabilities element"
    );
    let capabilities = capability_nodes[0];
    assert_eq!(
        capabilities
            .children()
            .filter(roxmltree::Node::is_element)
            .count(),
        0,
        "the AppContainer profile deliberately grants no capabilities"
    );
}
