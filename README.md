# clippyboardy

A lightweight Linux clipboard history manager in Rust + GTK 3 that works reliably on **GNOME Wayland** without remote-desktop permission prompts.

## Features

- **Text & image history** — Remembers up to 30 copied text snippets and PNG images (with cached thumbnails so the popup opens immediately), deduplicating entries and moving re-copied items to the top.
- **Mouse-relative popup** — Opens next to your cursor using an invisible pointer-capture overlay so Xwayland always knows the real pointer coordinates.
- **Smart screen-edge placement** — Flips to the left of the mouse near the right edge of the monitor, and shifts upward near the bottom of the screen so it sits beside the cursor and still reveals at least 10 entries (up to 15 when there is room below).
- **Fast navigation & cleanup** — Select with mouse click or arrow keys + `Enter`, dismiss with `Esc` or by clicking outside, hover or click the `▼` bar to scroll, and delete individual entries with the trash button.
- **Optional auto-paste** — Emits `Ctrl+V` through a kernel-level `/dev/uinput` virtual keyboard so GNOME does not pop up a "remote interaction" prompt.
- **Private local storage** — Persists history atomically under `${XDG_DATA_HOME:-~/.local/share}/clippyboardy` with `0700` directory and `0600` file permissions, automatically pruning removed image files.

## System Compatibility

**Best suited for:**
- **Linux desktops running GNOME on Wayland (with Xwayland)** — Built and tested for Ubuntu / GNOME Wayland (including Ubuntu 24.04+ / 26.04 LTS, Fedora Workstation, Debian GNOME, Arch GNOME, etc.).

**Also works on:**
- **Linux X11 desktop environments** (GNOME on X11, XFCE, MATE, Cinnamon, i3, Openbox, etc.). On non-GNOME desktops, skip the `gsettings` shortcut lines in `install.sh` and bind your desktop's custom shortcut to `~/.local/bin/clippyboardy` while autostarting `~/.local/bin/clippyboardy --daemon`.

**Not suitable for / limitations:**
- **Linux only** — Does not support Windows or macOS (depends on GTK 3 / X11 GDK APIs and Linux `/dev/uinput`).
- **Requires Xwayland (on Wayland) or X11** — Pure Wayland sessions without Xwayland are not supported, because `clippyboardy` intentionally uses the GTK X11 backend so it can monitor clipboard changes in the background on GNOME.
- **Auto-paste layout caveat** — Automatic `Ctrl+V` pasting sends physical Linux keycodes `KEY_LEFTCTRL` + `KEY_V` via `/dev/uinput`, which assumes a QWERTY physical `V` position. Copying the selected entry back to the clipboard still works on all keyboard layouts.

## Dependencies

- **Rust toolchain** (`cargo` and `rustc`)
- **GTK 3 development headers** and `pkg-config`:
  - Debian / Ubuntu: `sudo apt install libgtk-3-dev pkg-config`
  - Fedora: `sudo dnf install gtk3-devel pkgconf-pkg-config`
  - Arch: `sudo pacman -S gtk3 pkgconf`
- **Xwayland** (installed by default on GNOME Wayland desktops)

## Installation

### 1. Build, install, and bind `Super+V` (GNOME)

```bash
./install.sh
```

This script:
1. Compiles the release binary (`cargo build --release`).
2. Installs it to `~/.local/bin/clippyboardy`.
3. Adds an XDG autostart entry at `~/.config/autostart/clippyboardy.desktop` (`clippyboardy --daemon`).
4. Registers a GNOME custom shortcut binding `Super+V` to `~/.local/bin/clippyboardy`.
5. Starts the background daemon immediately.

### 2. Optional: Enable auto-paste via `/dev/uinput`

If your user does not have write access to `/dev/uinput`, `clippyboardy` will still work — selecting an item places it on your clipboard and you can press `Ctrl+V` manually.

To enable automatic pasting on selection, grant the active local desktop user access to `/dev/uinput` with a udev rule:

```bash
echo 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"' | \
  sudo tee /etc/udev/rules.d/70-uinput-uaccess.rules
sudo udevadm control --reload-rules
sudo udevadm trigger /dev/uinput
```

Then restart the daemon so it picks up `/dev/uinput`:

```bash
pkill -x clippyboardy || true
setsid -f ~/.local/bin/clippyboardy --daemon >/dev/null 2>&1
```

## Usage

- Copy text or images as usual — the background daemon records them automatically.
- Press **`Super+V`** to open the clipboard picker next to the mouse cursor.
- **Click** an entry (or move with arrow keys and press **`Enter`**) to copy it to the top of the clipboard and auto-paste it.
- Click the **trash icon** on a row to permanently remove that entry (and delete its stored PNG if it is an image).
- Press **`Esc`** or click outside the popup to close it.

## How It Works

- **Background clipboard watching**: GNOME does not expose the Wayland `wlr-data-control` protocol. Running GTK 3 on the X11 backend (`gdk::set_allowed_backends("x11")`) lets the daemon receive `owner-change` clipboard notifications via Xwayland while unfocused.
- **Accurate cursor position**: Because Xwayland only updates the pointer position over X windows, opening the picker briefly shows a transparent fullscreen popup to read the exact pointer coordinates from the enter/motion event before positioning the menu.
- **Focus & paste**: Uses `gdk_x11_get_server_time` / `gdk_x11_window_set_user_time` to work with GNOME focus-stealing prevention, then emits `Ctrl+V` via `/dev/uinput` after returning focus to the previous window.

## Development

```bash
cargo test
cargo build --release
```

Note: The `uinput_keyboard_creates` test is marked `#[ignore]` by default because it opens a real `/dev/uinput` virtual keyboard device.
