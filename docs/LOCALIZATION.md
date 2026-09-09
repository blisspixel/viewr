# Localization

viewr bundles its interface catalogs and never downloads language data. The
default preference is System. A user can instead choose English, Spanish,
French, or German in File > Preferences, and the choice applies immediately and
persists in the platform configuration directory.

## Platform resolution

- Windows reads the current user locale through the bounded native locale-name
  API.
- macOS reads the current `NSLocale` identifier.
- Linux checks `LC_ALL`, `LC_MESSAGES`, and `LANG` in that order.
- Region, encoding, and modifier suffixes do not change the primary language.
- An unsupported, missing, or malformed system locale resolves to English.

No locale value is logged, persisted as activity, or sent anywhere. The saved
preference is one validated word. Missing state quietly uses System. Invalid,
oversized, or unreadable state fails to System with path-free recovery guidance.

## Catalog boundary

The bundled catalog covers the primary menu bar, file and folder entry points,
Preferences, file-association entry points, empty-state actions, crop controls,
the main panel headings, the complete first-run, open-status, and Help shortcut
surface, and every Trash, permanent-delete, and restore message. Remaining
advanced status, metadata, and editing explanations still use the explicit
English fallback. The roadmap keeps complete catalog coverage and native
assistive-technology review as open work before localization can be called
complete. Destructive-action copy tells someone whether their file still
exists, so it needs native review before the accessibility and language matrix
is recorded.

Source strings have one catalog lookup boundary in `locale.rs`. Language
selection never branches through platform UI code, and missing catalog entries
fall back to the exact English source string rather than becoming blank.

That fallback is silent by design, so coverage is enforced mechanically instead
of by review:

- Interface literals bind through `tr!`. A literal that is not in the catalog
  fails the build, so new copy cannot ship translated in English only.
- `locale::interface_literals_bind_to_the_catalog` rejects a literal passed to
  the lookup directly, which would step around that build check.
- Copy that a pure seam supplies as a value cannot use `tr!`, so each such seam
  proves its own coverage by enumerating every string it can emit.
  `shortcuts::every_visible_string_is_cataloged` and
  `curation_state::every_curation_message_is_cataloged` do this. A seam that is
  not yet cataloged has no such test; adding one is how that surface is
  migrated.
- A seam that composes sentences from counts and names also proves the result.
  `curation_state::composed_copy_is_translated_and_fully_substituted` builds
  every message it can produce in all four languages, then requires each
  translated string to differ from its English source and to contain no
  leftover placeholder brace.

A user-visible sentence has exactly one owner. The empty-state card and the top
status line share `shortcuts::open_status`, and a test asserts they agree in
every language rather than formatting the same sentence twice.

## Adding a language

1. Add the language and its stable preference word to `Preference` and
   `Language`.
2. Map the primary locale subtag in `resolve_locale`.
3. Add a nonempty translation for every catalog entry. A parameterized entry
   keeps its `{subject}` placeholder so the substitution happens after
   translation and the language decides where the name belongs.
4. Verify native language names, menu widths, modal height, accented glyphs,
   keyboard shortcuts, accessible names, and polite status behavior.
5. Complete keyboard-only and native assistive-technology checks on Windows,
   macOS, and Linux using an exact candidate artifact.

Do not translate shortcuts, filenames, metadata values, format identifiers, or
product names. Do translate action names and the explanatory copy that gives
those values meaning.
