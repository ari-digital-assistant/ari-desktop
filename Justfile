# Ari for Linux: task runner. `just` with no arguments lists the recipes.

app_id := "dev.heyari.Ari"

# Show the recipes.
default:
    @just --list

# Debug build.
build:
    cargo build

# Run from the source tree, with the settings schema, translations and voice
# files it needs.
run *ARGS: build schemas locales voice
    GSETTINGS_SCHEMA_DIR=target/gsettings target/debug/ari-gtk {{ARGS}}

# Install a development copy for this user, so Ari opens from the launcher:
# the launcher entry (pointing at target/debug), the icons and the settings
# schema. `just uninstall-dev` takes it out again.
install-dev: build locales voice
    #!/usr/bin/env bash
    set -euo pipefail
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    install -Dm644 packaging/icons/hicolor/scalable/apps/{{app_id}}.svg "$data/icons/hicolor/scalable/apps/{{app_id}}.svg"
    install -Dm644 packaging/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg "$data/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg"
    just _icon-cache "$data/icons/hicolor"
    install -Dm644 packaging/gsettings/{{app_id}}.gschema.xml "$data/glib-2.0/schemas/{{app_id}}.gschema.xml"
    glib-compile-schemas "$data/glib-2.0/schemas"
    mkdir -p "$data/applications"
    sed "s|^Exec=ari-gtk|Exec=\"$PWD/target/debug/ari-gtk\"|" packaging/desktop/{{app_id}}.desktop > "$data/applications/{{app_id}}.desktop"
    echo "Installed. Ari opens from the launcher and runs target/debug/ari-gtk."

# Take out what `just install-dev` installed, and the autostart entry Ari
# wrote for itself.
uninstall-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    config="${XDG_CONFIG_HOME:-$HOME/.config}"
    rm -f "$data/applications/{{app_id}}.desktop" \
        "$data/icons/hicolor/scalable/apps/{{app_id}}.svg" \
        "$data/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg" \
        "$data/glib-2.0/schemas/{{app_id}}.gschema.xml" \
        "$config/autostart/{{app_id}}.desktop"
    just _icon-cache "$data/icons/hicolor"
    glib-compile-schemas "$data/glib-2.0/schemas"
    echo "Uninstalled. Lines Ari added to a Hyprland config stay; they sit between '>>> Ari' and '<<< Ari'."

# GTK trusts an icon cache over the files beside it, and adding an icon in a
# subfolder doesn't make the cache look stale. Another app may have left one in
# the user's hicolor folder, so rebuild it if it's there.
_icon-cache DIR:
    if [ -f "{{DIR}}/icon-theme.cache" ]; then gtk4-update-icon-cache --quiet --force --ignore-theme-index "{{DIR}}"; fi

# Compile the settings schema into target/gsettings, for `just run`.
schemas:
    mkdir -p target/gsettings
    glib-compile-schemas --strict --targetdir target/gsettings packaging/gsettings

# The engine's native voice library, with sherpa-onnx and ONNX Runtime, beside
# target/debug/ari-gtk, the wake word and speech detector models in
# target/share/ari/voice and Ari's sounds in target/share/ari/sounds, where it
# looks for them. Built from the engine checkout at ENGINE; the first build
# downloads TFLite Micro, ONNX Runtime's headers and sherpa-onnx.
voice ENGINE="../ari-engine":
    "{{ENGINE}}/native/voice/build-host.sh"
    mkdir -p target/debug target/share/ari/voice
    cp "{{ENGINE}}"/target/voice-native/lib*.so target/debug/
    cp "{{ENGINE}}"/assets/voice/*.onnx "{{ENGINE}}"/assets/voice/*.tflite "{{ENGINE}}"/assets/voice/*.json target/share/ari/voice/
    mkdir -p target/share/ari/sounds
    cp packaging/sounds/*.wav target/share/ari/sounds/

# Compile the translations into target/share/locale, where a binary in
# target/debug looks for them.
locales:
    #!/usr/bin/env bash
    set -euo pipefail
    while read -r lang; do
        mkdir -p "target/share/locale/$lang/LC_MESSAGES"
        msgfmt --check -o "target/share/locale/$lang/LC_MESSAGES/ari-desktop.mo" "po/$lang.po"
    done < po/LINGUAS

# Write the translation template, po/ari-desktop.pot, and merge it into every
# translation.
pot:
    #!/usr/bin/env bash
    set -euo pipefail
    just _template po/ari-desktop.pot
    while read -r lang; do
        msgmerge --quiet --update --backup=none --no-fuzzy-matching "po/$lang.po" po/ari-desktop.pot
    done < po/LINGUAS

_template OUT:
    xgettext --files-from=po/POTFILES --directory=. --output={{OUT}} \
        --from-code=UTF-8 --keyword=gettext --keyword=ngettext:1,2 --add-comments=Translators \
        --package-name=ari-desktop \
        --msgid-bugs-address=https://github.com/ari-digital-assistant/ari-desktop/issues

# Format check, clippy (warnings are errors), tests, packaging files and
# translations. What CI runs.
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    just check-packaging
    just check-translations

# Validate the launcher entry, the AppStream metainfo and the settings schema.
check-packaging:
    desktop-file-validate packaging/desktop/*.desktop
    appstreamcli validate --no-net packaging/{{app_id}}.metainfo.xml
    glib-compile-schemas --strict --dry-run packaging/gsettings

# Every translation covers every string in the current source.
check-translations:
    #!/usr/bin/env bash
    set -euo pipefail
    template="$(mktemp --suffix=.pot)"
    trap 'rm -f "$template"' EXIT
    just _template "$template"
    while read -r lang; do
        msgfmt --check -o /dev/null "po/$lang.po"
        msgcmp "po/$lang.po" "$template"
    done < po/LINGUAS
