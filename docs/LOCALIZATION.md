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

The initial bundled catalog covers the primary menu bar, file and folder entry
points, Preferences, file-association entry points, empty-state actions, crop
controls, and the main panel headings. Advanced status, recovery, metadata, and
editing explanations currently use the explicit English fallback. The roadmap
keeps complete catalog coverage and native assistive-technology review as open
work before localization can be called complete.

Source strings have one catalog lookup boundary in `locale.rs`. Language
selection never branches through platform UI code, and missing catalog entries
fall back to the exact English source string rather than becoming blank.

## Adding a language

1. Add the language and its stable preference word to `Preference` and
   `Language`.
2. Map the primary locale subtag in `resolve_locale`.
3. Add a nonempty translation for every catalog entry.
4. Verify native language names, menu widths, modal height, accented glyphs,
   keyboard shortcuts, accessible names, and polite status behavior.
5. Complete keyboard-only and native assistive-technology checks on Windows,
   macOS, and Linux using an exact candidate artifact.

Do not translate shortcuts, filenames, metadata values, format identifiers, or
product names. Do translate action names and the explanatory copy that gives
those values meaning.
