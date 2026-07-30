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


if __name__ == "__main__":
    unittest.main()
