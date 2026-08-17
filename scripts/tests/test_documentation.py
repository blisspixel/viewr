"""Repository documentation contract tests."""

from __future__ import annotations

from pathlib import Path
import re
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
            "README.md": (
                "v0.1.5 is the current public preview",
                "checksummed and",
                "attested",
                "not Authenticode-signed",
                "notarized",
            ),
            "docs/INSTALL.md": (
                "v0.1.5 is the current public GitHub Release",
                "checksummed",
                "manifest-verified",
                "attested",
                "not Authenticode-signed",
                "ID-signed or notarized",
            ),
            "docs/ROADMAP.md": (
                "Current position: v0.1.5 is released and verified",
                "Public foundation, released",
                "immutable checksummed archives",
                "attestations",
                "explicit unsigned-preview limits",
            ),
            "docs/PUBLISHING.md": (
                "v0.1.5 is public, immutable, checksummed, and attested",
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
            "CHANGELOG.md": (
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

    def test_pre_one_version_path_builds_product_before_distribution(self) -> None:
        roadmap = (REPOSITORY_ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
        standards = (REPOSITORY_ROOT / "docs/STANDARDS.md").read_text(encoding="utf-8")
        ordered_gates = (
            "| **v0.2.0** | Reliability architecture beta |",
            "| **v0.3.0** | Display-correct SDR preview |",
            "| **v0.4.0** | File-coherence preview |",
            "| **v0.5.0** | Format-contract preview |",
            "| **v0.6.0** | Integrated product-quality beta |",
            "| **v0.7.0** | Accessibility evidence preview |",
            "| **v0.8.0** | Release-readiness beta |",
            "| **v0.9.0** | Publisher-authenticated release candidate |",
        )
        positions = [roadmap.index(gate) for gate in ordered_gates]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("Immediate focus: v0.2 reliability architecture", roadmap)
        self.assertIn("Native platform trust | Deferred to v0.9", roadmap)
        self.assertIn("explicitly unsigned pre-1.0 preview", standards)
        self.assertIn("Publisher authentication remains a v0.9 and 1.0 gate", standards)


if __name__ == "__main__":
    unittest.main()
