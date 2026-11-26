#!/usr/bin/env bash
# Builds, installs to ~/.local/bin, autostarts at login, and binds Super+V to open the picker.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/clippyboardy ~/.local/bin/clippyboardy

mkdir -p ~/.config/autostart
cat > ~/.config/autostart/clippyboardy.desktop <<DESKTOP
[Desktop Entry]
Type=Application
Name=Clippyboardy
Exec=$HOME/.local/bin/clippyboardy --daemon
X-GNOME-Autostart-enabled=true
DESKTOP

# Append our shortcut to the existing custom ones instead of replacing them.
KEY=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/clippyboardy/
LIST=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings)
if [[ $LIST != *"$KEY"* ]]; then
  if [[ $LIST == "@as []" || $LIST == "[]" ]]; then LIST="['$KEY']"; else LIST="${LIST%]}, '$KEY']"; fi
  gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$LIST"
fi
S=org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$KEY
gsettings set "$S" name 'Clippyboardy'
gsettings set "$S" command "$HOME/.local/bin/clippyboardy"
gsettings set "$S" binding '<Super>v'

pkill -x clippyboardy || true
setsid -f ~/.local/bin/clippyboardy --daemon >/dev/null 2>&1
echo "Installed. Press Super+V to open clipboard history."
