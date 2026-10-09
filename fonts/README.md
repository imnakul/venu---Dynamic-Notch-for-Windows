# Bundled fonts

Venu embeds the variable Geist and Geist Mono TrueType fonts from the Google
Fonts repository's OFL directories, along with Noto Sans Devanagari as an egui
fallback. The exact source URLs, byte lengths, and SHA-256 hashes are recorded
in [SOURCE.json](SOURCE.json). Geist licenses are included beside their files;
the Noto font remains at the repository root to preserve its existing include
paths, and its license is included here. Font files are loaded from memory by
DirectWrite and egui; Venu does not install them into Windows.
