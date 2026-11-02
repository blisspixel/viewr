"""Repository documentation contract tests."""

from __future__ import annotations

from pathlib import Path
import re
import subprocess
import unittest
from urllib.parse import unquote


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
MARKDOWN_LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")


class DocumentationTests(unittest.TestCase):
    """Keep canonical local links valid on case-sensitive public checkouts."""

    @staticmethod
    def canonical_markdown_files() -> list[Path]:
        roots = [
            *REPOSITORY_ROOT.glob("*.md"),
            *(REPOSITORY_ROOT / "docs").glob("*.md"),
            *(REPOSITORY_ROOT / ".github").glob("*.md"),
        ]
        return sorted(path for path in roots if path.is_file())

    @staticmethod
    def has_exact_case(path: Path) -> bool:
        relative = path.relative_to(REPOSITORY_ROOT)
        current = REPOSITORY_ROOT
        for part in relative.parts:
            matches = [entry.name for entry in current.iterdir() if entry.name == part]
            if matches != [part]:
                return False
            current /= part
        return True

    def test_local_markdown_links_resolve_with_exact_case(self) -> None:
        failures: list[str] = []
        for document in self.canonical_markdown_files():
            contents = document.read_text(encoding="utf-8")
            for raw_target in MARKDOWN_LINK.findall(contents):
                target = raw_target.strip()
                if target.startswith("<") and ">" in target:
                    target = target[1 : target.index(">")]
                else:
                    target = target.split(maxsplit=1)[0]
                target = unquote(target.split("#", 1)[0])
                if not target or "://" in target or target.startswith("mailto:"):
                    continue
                resolved = (document.parent / target).resolve()
                try:
                    resolved.relative_to(REPOSITORY_ROOT)
                except ValueError:
                    failures.append(
                        f"{document}: link escapes the repository: {target}"
                    )
                    continue
                if not resolved.is_file():
                    failures.append(f"{document}: missing local link target: {target}")
                elif not self.has_exact_case(resolved):
                    failures.append(
                        f"{document}: link has incorrect filename case: {target}"
                    )
        self.assertEqual(failures, [], "\n" + "\n".join(failures))

    def test_public_release_state_is_consistent(self) -> None:
        requirements = {
            ".github/ISSUE_TEMPLATE/bug_report.yml": ("0.7.7 or commit SHA",),
            "README.md": (
                "v0.7.7 is the preview described here",
                "checksummed and",
                "attested",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/INSTALL.md": (
                "v0.7.7 is the preview described here",
                "checksummed",
                "manifest-verified",
                "attested",
                "not Authenticode-signed",
                "ID-signed or notarized",
            ),
            "docs/ROADMAP.md": (
                "Published install target | Immutable [v0.7.7]",
                "Public foundation, released",
                "immutable checksummed archives",
                "attestations",
                "explicit unsigned-preview limits",
            ),
            "docs/PUBLISHING.md": (
                "v0.7.7 is public, immutable, checksummed, and attested",
                "explicitly unsigned pre-1.0 preview",
            ),
            "docs/releases/v0.1.0.md": (
                "The first public preview of viewr",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "Known issues in this release",
            ),
            "docs/releases/v0.1.1.md": (
                "A patch release over the first public preview",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.1.2.md": (
                "A patch release over [v0.1.1]",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.1.3.md": (
                "A patch release over [v0.1.2]",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.1.4.md": (
                "A patch release over [v0.1.3]",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.1.5.md": (
                "A patch release over [v0.1.4]",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.2.0.md": (
                "reliability architecture",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.3.0.md": (
                "display-correct SDR",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.4.0.md": (
                "file-coherence",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.5.0.md": (
                "format-contract",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/releases/v0.6.0.md": (
                "integrated product-quality beta",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "security controls to launch viewr",
                "product-quality matrix was not completed for",
            ),
            "docs/releases/v0.6.1.md": (
                "shipped-feedback patch",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware product-quality matrix is still incomplete",
            ),
            "docs/releases/v0.6.2.md": (
                "shipped-feedback patch",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware product-quality matrix is still incomplete",
            ),
            "docs/releases/v0.6.3.md": (
                "shipped-feedback patch",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware product-quality matrix is still incomplete",
            ),
            "docs/releases/v0.6.4.md": (
                "shipped-feedback patch",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware product-quality matrix is still incomplete",
            ),
            "docs/releases/v0.6.5.md": (
                "shipped-feedback patch",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware product-quality matrix is still incomplete",
            ),
            "docs/releases/v0.7.0.md": (
                "accessibility and collage preview",
                "GitHub build-provenance attestation",
                "not Authenticode-signed",
                "notarized",
                "representative-hardware matrix",
            ),
            "docs/releases/v0.7.1.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.2.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.3.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.4.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.5.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.6.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.7.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "docs/releases/v0.7.8.md": (
                "patch over",
                "GitHub build-provenance attestations",
                "not Authenticode-signed",
                "notarized",
                "Representative-hardware",
            ),
            "CHANGELOG.md": (
                "## 0.7.7 - 2026-11-01",
                "## 0.7.6 - 2026-10-24",
                "## 0.7.5 - 2026-10-21",
                "## 0.7.4 - 2026-10-03",
                "## 0.7.3 - 2026-10-03",
                "## 0.7.2 - 2026-09-29",
                "## 0.7.1 - 2026-09-28",
                "## 0.7.0 - 2026-09-28",
                "## 0.6.5 - 2026-09-25",
                "## 0.6.4 - 2026-09-24",
                "## 0.6.3 - 2026-09-09",
                "## 0.6.2 - 2026-09-09",
                "## 0.6.1 - 2026-09-02",
                "## 0.6.0 - 2026-08-29",
                "## 0.5.0 - 2026-08-20",
                "## 0.4.0 - 2026-08-19",
                "## 0.3.0 - 2026-08-19",
                "## 0.2.0 - 2026-08-18",
                "## 0.1.5 - 2026-08-17",
                "## 0.1.4 - 2026-08-17",
                "## 0.1.3 - 2026-08-16",
                "## 0.1.2 - 2026-08-15",
                "## 0.1.1 - 2026-08-15",
                "## 0.1.0 - 2026-07-31",
                "first unsigned pre-1.0 release",
            ),
        }
        contents: dict[str, str] = {}
        for relative_path, required_phrases in requirements.items():
            document = REPOSITORY_ROOT / relative_path
            contents[relative_path] = document.read_text(encoding="utf-8")
            for phrase in required_phrases:
                with self.subTest(document=relative_path, phrase=phrase):
                    self.assertIn(phrase, contents[relative_path])

        combined = "\n".join(contents.values()).casefold()
        stale_claims = (
            "no public github release exists",
            "build from source today",
            "become active with the first tagged release",
            "planned first portable archives",
        )
        for stale_claim in stale_claims:
            with self.subTest(stale_claim=stale_claim):
                self.assertNotIn(stale_claim, combined)

    def test_quality_commands_match_the_executable_ci_contract(self) -> None:
        core_commands = (
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --locked -- -D warnings",
            "cargo test --workspace --all-targets --locked",
        )
        contract_surfaces = (
            "README.md",
            "CONTRIBUTING.md",
            "AGENTS.md",
            "docs/STANDARDS.md",
            "docs/VERIFY.md",
            ".github/workflows/ci.yml",
        )
        for relative_path in contract_surfaces:
            contents = (REPOSITORY_ROOT / relative_path).read_text(encoding="utf-8")
            for command in core_commands:
                with self.subTest(document=relative_path, command=command):
                    self.assertIn(command, contents)

        workflow = (REPOSITORY_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        verification_guide = (REPOSITORY_ROOT / "docs/VERIFY.md").read_text(
            encoding="utf-8"
        )
        workflow_commands = (
            "cargo test --workspace --doc --locked",
            "cargo build --workspace --all-targets --locked",
            (
                "cargo deny --locked check --hide-inclusion-graph -D warnings "
                "-A license-not-encountered -A unmatched-skip "
                "-A unnecessary-skip"
            ),
            (
                "cargo deny --manifest-path fuzz/Cargo.toml --locked check "
                "--hide-inclusion-graph -D warnings"
            ),
        )
        for command in workflow_commands:
            with self.subTest(command=command):
                self.assertIn(command, workflow)
                self.assertIn(command, verification_guide)
        self.assertIn(
            "- name: Test\n        run: cargo test --workspace --all-targets --locked",
            workflow,
        )
        self.assertIn(
            "- name: Documentation tests\n"
            "        run: cargo test --workspace --doc --locked",
            workflow,
        )
        self.assertRegex(workflow, r"cargo llvm-cov --workspace\s+--locked")
        self.assertEqual(workflow.count("--exclude jxl-color"), 1)
        self.assertEqual(workflow.count("--exclude jxl-render"), 1)
        exclusion_matches = re.findall(
            r"--ignore-filename-regex '([^']+)'",
            workflow,
        )
        self.assertEqual(len(exclusion_matches), 1)
        exclusion = re.compile(exclusion_matches[0])
        rust_files = {
            path.relative_to(REPOSITORY_ROOT).as_posix()
            for root in ("crates", "fuzz", "vendor")
            for path in (REPOSITORY_ROOT / root).rglob("*.rs")
        }
        excluded = {path for path in rust_files if exclusion.search(path)}
        self.assertEqual(
            excluded,
            {
                "crates/viewr-decode/src/main.rs",
                "crates/viewr/src/app.rs",
                "crates/viewr/src/gpu.rs",
                "crates/viewr/src/main.rs",
                "crates/viewr/src/sandbox.rs",
                "crates/viewr/src/worker_limit.rs",
            },
        )

        privacy_command_parts = (
            "cargo deny --locked check --hide-inclusion-graph -D warnings",
            "-A license-not-encountered -A unmatched-skip -A unnecessary-skip",
        )
        for relative_path in (
            "scripts/privacy-check.ps1",
            "scripts/privacy-check.sh",
        ):
            contents = (REPOSITORY_ROOT / relative_path).read_text(encoding="utf-8")
            for command_part in privacy_command_parts:
                with self.subTest(document=relative_path, command=command_part):
                    self.assertIn(command_part, contents)

        deny_policy = (REPOSITORY_ROOT / "deny.toml").read_text(encoding="utf-8")
        self.assertIn('multiple-versions = "deny"', deny_policy)
        self.assertNotIn('multiple-versions = "warn"', deny_policy)

    def test_agents_md_starts_and_stops_tidy(self) -> None:
        agents = (REPOSITORY_ROOT / "AGENTS.md").read_text(encoding="utf-8")
        self.assertIn("### Start tidy, stop tidy", agents)
        self.assertIn("git fetch --prune", agents)
        self.assertIn("check PR state, not only", agents)
        self.assertIn("When committing and pushing are in scope", agents)
        self.assertIn("Never use", agents)
        self.assertIn("`--force` to remove one", agents)
        self.assertLess(
            agents.index("## Autonomy"),
            agents.index("### Start tidy, stop tidy"),
        )

    def test_claude_md_imports_canonical_agents_md(self) -> None:
        claude = (REPOSITORY_ROOT / "CLAUDE.md").read_text(encoding="utf-8")
        self.assertIn("@AGENTS.md", claude)
        for command in (
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --locked -- -D warnings",
            "cargo test --workspace --all-targets --locked",
        ):
            with self.subTest(command=command):
                self.assertNotIn(command, claude)

    def test_linux_runtime_libraries_match_the_launch_check(self) -> None:
        """Every library the launch check probes is named in the install guide."""
        startup = (REPOSITORY_ROOT / "crates/viewr/src/startup.rs").read_text(
            encoding="utf-8"
        )
        sonames = re.findall(r'sonames: &\["([^"]+)"', startup)
        debian_packages = re.findall(r'debian: "([^"]+)"', startup)
        self.assertEqual(len(sonames), 6)
        self.assertEqual(len(debian_packages), len(sonames))

        install = (REPOSITORY_ROOT / "docs/INSTALL.md").read_text(encoding="utf-8")
        for soname in sonames:
            with self.subTest(soname=soname):
                self.assertIn(soname, install)
        for package in debian_packages:
            with self.subTest(package=package):
                self.assertIn(package, install)
        self.assertIn("`viewr doctor` checks the windowing libraries", install)

    def test_documentation_never_instructs_a_platform_security_bypass(self) -> None:
        documents = [
            *REPOSITORY_ROOT.glob("*.md"),
            *(REPOSITORY_ROOT / "docs").rglob("*.md"),
            *(REPOSITORY_ROOT / ".github").rglob("*.md"),
            *(REPOSITORY_ROOT / "packaging").rglob("*.md"),
        ]
        forbidden = (
            "spctl --master-disable",
            "xattr -dr com.apple.quarantine",
            "set-executionpolicy bypass",
            "set-executionpolicy unrestricted",
            "--no-sandbox",
        )
        for document in sorted(set(documents)):
            contents = document.read_text(encoding="utf-8").casefold()
            for instruction in forbidden:
                with self.subTest(document=document, instruction=instruction):
                    self.assertNotIn(instruction, contents)

    def test_pre_one_version_path_builds_product_before_distribution(self) -> None:
        roadmap = (REPOSITORY_ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
        standards = (REPOSITORY_ROOT / "docs/STANDARDS.md").read_text(encoding="utf-8")
        publishing = (REPOSITORY_ROOT / "docs/PUBLISHING.md").read_text(
            encoding="utf-8"
        )
        ordered_gates = (
            "| **v0.2.0** | Reliability architecture beta |",
            "| **v0.3.0** | Display-correct SDR preview |",
            "| **v0.4.0** | File-coherence preview |",
            "| **v0.5.0** | Format-contract preview |",
            "| **v0.6.0** | Integrated product-quality beta |",
            "| **v0.7.0** | Accessibility preview |",
            "| **v0.8.0** | Release-readiness beta |",
            "| **v0.9.0** | Publisher-authenticated release candidate |",
        )
        positions = [roadmap.index(gate) for gate in ordered_gates]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("Immediate focus: v0.8 release readiness", roadmap)
        self.assertIn("### Next steps to v0.8.0", roadmap)
        self.assertIn("### Version state and update points", roadmap)
        self.assertIn("Native platform trust | Deferred to v0.9", roadmap)
        self.assertIn("explicitly unsigned pre-1.0 preview", standards)
        self.assertIn("Publisher authentication remains a v0.9 and 1.0 gate", standards)
        self.assertIn("## Version state policy", publishing)
        self.assertIn("full commit SHA plus one non-publishing", publishing)
        self.assertLess(
            publishing.index("`docs/releases/v<version>.md`"),
            publishing.index("git tag -a v0.8.0"),
        )
        self.assertNotIn("git tag -a v0.6.0", publishing)

    def test_roadmap_uses_gates_instead_of_schedule_estimates(self) -> None:
        roadmap = (REPOSITORY_ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
        self.assertIn("There are no calendar promises or duration estimates", roadmap)
        for estimate in (
            "will take",
            "should take",
            "estimated time",
            "target date",
            "eta:",
        ):
            with self.subTest(estimate=estimate):
                self.assertNotIn(estimate, roadmap.casefold())

    def test_first_run_and_help_copy_stay_one_catalog(self) -> None:
        shortcuts = (REPOSITORY_ROOT / "crates/viewr/src/shortcuts.rs").read_text(
            encoding="utf-8"
        )
        match = re.search(r'FIRST_RUN_SCOPE: &str = "([^"]+)";', shortcuts)
        self.assertIsNotNone(match, "missing FIRST_RUN_SCOPE string")
        scope = match.group(1)
        self.assertIn("drop a file or folder", scope)

        for relative_path in (
            "docs/PRODUCT-QUALITY.md",
            "scripts/accessibility-smoke.ps1",
        ):
            contents = (REPOSITORY_ROOT / relative_path).read_text(encoding="utf-8")
            with self.subTest(document=relative_path):
                self.assertIn(scope, contents)

        readme = (REPOSITORY_ROOT / "README.md").read_text(encoding="utf-8")
        self.assertNotIn("`A`/`D`", readme)
        self.assertIn("`[` / `]`", readme)
        self.assertIn("Space tap fits", readme)

        product_quality = (REPOSITORY_ROOT / "docs/PRODUCT-QUALITY.md").read_text(
            encoding="utf-8"
        )
        self.assertIn("It does not close v0.6", product_quality)
        self.assertIn(
            "Do not claim representative-hardware acceptance", product_quality
        )
        roadmap = (REPOSITORY_ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
        self.assertIn("PRODUCT-QUALITY.md", roadmap)
        release_artifact = (REPOSITORY_ROOT / "scripts/release_artifact.py").read_text(
            encoding="utf-8"
        )
        self.assertIn('"docs/PRODUCT-QUALITY.md"', release_artifact)
        self.assertIn("Candidate workflow run", product_quality)
        self.assertIn("scripts/product_quality_evidence.py gate", product_quality)
        self.assertIn(
            "downloads that\nrun into a fresh temporary directory", product_quality
        )
        self.assertIn("product-quality-fixtures", product_quality)
        self.assertIn("gen_product_quality_fixtures", product_quality)

    def test_macos_baseline_stays_separate_from_evidence(self) -> None:
        """Intent, declared floors, and missing hardware receipts stay distinct."""
        install = (REPOSITORY_ROOT / "docs/INSTALL.md").read_text(encoding="utf-8")
        architecture = (REPOSITORY_ROOT / "docs/ARCHITECTURE.md").read_text(
            encoding="utf-8"
        )
        product_quality = (REPOSITORY_ROOT / "docs/PRODUCT-QUALITY.md").read_text(
            encoding="utf-8"
        )
        plist = (REPOSITORY_ROOT / "packaging/macos/Info.plist").read_text(
            encoding="utf-8"
        )
        toolchain = (REPOSITORY_ROOT / "rust-toolchain.toml").read_text(
            encoding="utf-8"
        )
        lock = (REPOSITORY_ROOT / "Cargo.lock").read_text(encoding="utf-8")
        ci = (REPOSITORY_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        called = (REPOSITORY_ROOT / ".github/workflows/dependabot-ci.yml").read_text(
            encoding="utf-8"
        )
        release = (REPOSITORY_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        channel = re.search(r'channel = "([^"]+)"', toolchain)
        self.assertIsNotNone(channel, "missing toolchain channel")
        assert channel is not None
        self.assertIn(f"pinned Rust toolchain, {channel.group(1)},", install)

        for package, documented in (("winit", "0.30.13"), ("wgpu-hal", "29.0.4")):
            version = self.package_version(lock, package)
            self.assertEqual(version, documented)
            self.assertIn(f"{package} {version}", install)

        for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
            deployment = self.apple_deployment_target(target)
            self.assertIn(f"`{target}` at deployment target {deployment}", install)
            self.assertIn(f"- os: macos-latest\n            target: {target}", release)

        self.assertIn(
            "<key>LSMinimumSystemVersion</key>\n  <string>12.0</string>", plist
        )
        self.assertIn("strictest declared floor is macOS 12.0", install)
        self.assertIn("declared floor is the 11.0 link target", install)
        self.assertIn("not a runtime qualification", install)
        self.assertIn("original M1", install)
        self.assertIn(
            "viewr-<version>-aarch64-apple-darwin.zip",
            install,
        )
        self.assertIn("viewr-<version>-x86_64-apple-darwin.zip", install)
        self.assertIn("Existing Intel releases remain", install)

        self.assertIn("fmt / clippy / test (macos-latest)", architecture)
        self.assertIn("arm64 macOS 26", architecture)
        self.assertIn("not an original M1", architecture)
        self.assertIn("[install guide](INSTALL.md)", architecture)
        self.assertIn("os: [ubuntu-latest, macos-latest, windows-latest]", ci)
        self.assertIn("os: [ubuntu-latest, macos-latest, windows-latest]", called)
        self.assertIn("name: fmt / clippy / test (${{ matrix.os }})", ci)

        self.assertIn(
            "| macOS | Apple Silicon Retina laptop plus an external display, including a live move between them | Not yet recorded |",
            product_quality,
        )
        self.assertIn("Original M1 runtime qualification", product_quality)
        self.assertIn("do not fill either gap", product_quality)
        for claim in (
            "tested on an original M1",
            "qualified on original M1",
            "M1 acceptance passed",
        ):
            for document in (install, architecture, product_quality):
                with self.subTest(claim=claim):
                    self.assertNotIn(claim, document)

        assignment = "MACOSX_DEPLOYMENT_TARGET="
        for relative in (".github", "scripts", "packaging", "crates"):
            root = REPOSITORY_ROOT / relative
            for path in root.rglob("*"):
                if not path.is_file() or path.suffix.lower() not in {
                    ".yml",
                    ".yaml",
                    ".py",
                    ".sh",
                    ".ps1",
                    ".rs",
                    ".toml",
                    ".plist",
                }:
                    continue
                if path.name == "test_documentation.py":
                    continue
                text = path.read_text(encoding="utf-8")
                with self.subTest(path=path.relative_to(REPOSITORY_ROOT)):
                    self.assertNotIn(assignment, text)

    @staticmethod
    def package_version(lock_text: str, name: str) -> str:
        marker = f'name = "{name}"\nversion = "'
        start = lock_text.index(marker) + len(marker)
        return lock_text[start : lock_text.index('"', start)]

    @staticmethod
    def apple_deployment_target(target: str) -> str:
        completed = subprocess.run(
            ["rustc", "--print", "deployment-target", "--target", target],
            check=True,
            capture_output=True,
            text=True,
        )
        prefix = "MACOSX_DEPLOYMENT_TARGET="
        line = completed.stdout.strip()
        if not line.startswith(prefix):
            raise AssertionError(f"unexpected deployment target output: {line}")
        return line.removeprefix(prefix)


if __name__ == "__main__":
    unittest.main()
