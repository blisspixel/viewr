//! Repository-level checks for the three network-denied packaging profiles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use roxmltree::{Document, ParsingOptions};
use viewr::fs::{CORE_EXTENSIONS, CORE_MIME_ASSOCIATIONS};

const UAP_NAMESPACE: &str = "http://schemas.microsoft.com/appx/manifest/uap/windows10";
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

fn expected_core_extensions() -> BTreeSet<String> {
    CORE_EXTENSIONS
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect()
}

fn desktop_fields(source: &str) -> std::collections::BTreeMap<&str, &str> {
    let mut fields = std::collections::BTreeMap::new();
    for line in source
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(['#', '[']))
    {
        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("invalid desktop entry field: {line}"));
        assert!(
            fields.insert(key, value).is_none(),
            "duplicate desktop entry field: {key}"
        );
    }
    fields
}

fn plist_value_for_key<'a, 'input>(
    dictionary: roxmltree::Node<'a, 'input>,
    key: &str,
) -> roxmltree::Node<'a, 'input> {
    let elements: Vec<_> = dictionary
        .children()
        .filter(roxmltree::Node::is_element)
        .collect();
    let key_index = elements
        .iter()
        .position(|node| node.has_tag_name("key") && node.text() == Some(key))
        .unwrap_or_else(|| panic!("plist key missing: {key}"));
    elements
        .get(key_index + 1)
        .copied()
        .unwrap_or_else(|| panic!("plist value missing: {key}"))
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
        "install -Dm644 assets/linux/viewr.desktop /app/share/applications/com.github.blisspixel.viewr.desktop",
        "install -Dm644 assets/icon.svg /app/share/icons/hicolor/scalable/apps/com.github.blisspixel.viewr.svg",
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
fn linux_desktop_entry_advertises_only_core_formats_without_taking_over_defaults() {
    let source = read_repository_file("assets/linux/viewr.desktop");
    let fields = desktop_fields(&source);
    assert_eq!(fields.get("Exec"), Some(&"viewr %f"));
    assert_eq!(fields.get("Icon"), Some(&"com.github.blisspixel.viewr"));
    assert_eq!(fields.get("Terminal"), Some(&"false"));

    let mime_types: Vec<_> = fields
        .get("MimeType")
        .expect("desktop entry declares MIME types")
        .split(';')
        .filter(|value| !value.is_empty())
        .collect();
    let actual: BTreeSet<_> = mime_types.iter().copied().collect();
    let expected = CORE_MIME_ASSOCIATIONS
        .iter()
        .map(|(_, mime_type)| *mime_type)
        .collect();
    assert_eq!(mime_types.len(), actual.len(), "duplicate MIME type");
    assert_eq!(actual, expected, "desktop entry must match core formats");

    assert!(
        !source.contains("xdg-mime default"),
        "installing a desktop entry must not choose the user's default viewer"
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
fn macos_profile_registers_core_formats_as_an_alternate_viewer() {
    let source = read_repository_file("packaging/macos/Info.plist");
    let document = Document::parse_with_options(
        &source,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .expect("parse Info.plist");
    let root = document
        .descendants()
        .find(|node| node.has_tag_name("dict"))
        .expect("Info.plist has a root dictionary");
    let document_types = plist_value_for_key(root, "CFBundleDocumentTypes");
    assert!(document_types.has_tag_name("array"));
    let document_type_values: Vec<_> = document_types
        .children()
        .filter(|node| node.has_tag_name("dict"))
        .collect();
    assert_eq!(
        document_type_values.len(),
        1,
        "one exact document type is declared"
    );
    let document_type = document_type_values[0];

    let extensions = plist_value_for_key(document_type, "CFBundleTypeExtensions");
    let extension_values: Vec<_> = extensions
        .children()
        .filter(|node| node.has_tag_name("string"))
        .map(|node| node.text().expect("extension has text"))
        .collect();
    let actual: BTreeSet<_> = extension_values
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    assert_eq!(extension_values.len(), actual.len(), "duplicate extension");
    assert_eq!(actual, expected_core_extensions());
    assert_eq!(
        plist_value_for_key(document_type, "CFBundleTypeRole").text(),
        Some("Viewer")
    );
    assert_eq!(
        plist_value_for_key(document_type, "LSHandlerRank").text(),
        Some("Alternate")
    );
}

#[test]
fn macos_open_file_integration_preserves_winit_delegate_ownership() {
    let source = read_repository_file("crates/viewr/src/macos.rs");
    for required in [
        "WinitApplicationDelegate",
        "class_addMethod",
        "EventLoopProxy",
        "send_event(UserEvent::OpenFile(path))",
    ] {
        assert!(
            source.contains(required),
            "macOS open-file integration is missing: {required}"
        );
    }
    assert!(
        !source.contains("setDelegate("),
        "viewr must extend, never replace, winit's application delegate"
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

    let associations: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name((UAP_NAMESPACE, "FileTypeAssociation")))
        .collect();
    assert_eq!(associations.len(), 1, "one image association is declared");
    assert_eq!(associations[0].attribute("Name"), Some("core-images"));

    let extension_values: Vec<_> = associations[0]
        .descendants()
        .filter(|node| node.has_tag_name((UAP_NAMESPACE, "FileType")))
        .map(|node| node.text().expect("file type has text"))
        .collect();
    let actual: BTreeSet<_> = extension_values
        .iter()
        .map(|extension| {
            extension
                .strip_prefix('.')
                .expect("Windows file type begins with a dot")
                .to_owned()
        })
        .collect();
    assert_eq!(extension_values.len(), actual.len(), "duplicate file type");
    assert_eq!(actual, expected_core_extensions());
}
