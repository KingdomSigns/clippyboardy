use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib, prelude::*};
use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

const MAX: usize = 30;

#[derive(Clone)]
enum Entry {
    Text(String),
    /// PNG file name in the data dir, plus a thumbnail kept in memory so the picker opens instantly.
    Image(String, Option<Pixbuf>),
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Entry::Text(a), Entry::Text(b)) => a == b,
            (Entry::Image(a, _), Entry::Image(b, _)) => a == b, // file name is a content hash
            _ => false,
        }
    }
}

type History = Rc<RefCell<Vec<Entry>>>;

fn data_dir() -> PathBuf {
    glib::user_data_dir().join("clippyboardy")
}

fn ensure_dir() -> io::Result<PathBuf> {
    let dir = data_dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?; // history can hold secrets/screenshots
    Ok(dir)
}

fn thumb(file: &str) -> Option<Pixbuf> {
    Pixbuf::from_file_at_scale(data_dir().join(file), 240, 120, true).ok()
}

// File format: NUL-separated entries, each prefixed 'T' (text) or 'I' (image file name).
fn load() -> Vec<Entry> {
    fs::read_to_string(data_dir().join("history"))
        .unwrap_or_default()
        .split('\0')
        .filter_map(|e| match e.split_at_checked(1)? {
            ("T", t) => Some(Entry::Text(t.into())),
            ("I", f) if data_dir().join(f).exists() => Some(Entry::Image(f.into(), thumb(f))),
            _ => None,
        })
        .take(MAX)
        .collect()
}

fn save(h: &[Entry]) {
    let lines: Vec<String> = h
        .iter()
        .map(|e| match e {
            Entry::Text(t) => format!("T{t}"),
            Entry::Image(f, _) => format!("I{f}"),
        })
        .collect();
    let res = ensure_dir().and_then(|dir| {
        let tmp = dir.join("history.tmp");
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(lines.join("\0").as_bytes())?;
        fs::rename(&tmp, dir.join("history")) // atomic replace so a crash never truncates history
    });
    if let Err(e) = res {
        eprintln!("clippyboardy: saving history failed: {e}");
        return;
    }
    // Delete images that fell off the end of the list.
    for f in fs::read_dir(data_dir()).into_iter().flatten().flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !h.iter().any(|e| matches!(e, Entry::Image(n, _) if *n == name)) {
            let _ = fs::remove_file(f.path());
        }
    }
}

/// Moves `e` to the top, dropping duplicates and anything past MAX. Returns whether history changed.
fn push(h: &mut Vec<Entry>, e: Entry) -> bool {
    if matches!(&e, Entry::Text(t) if t.trim().is_empty()) || h.first() == Some(&e) {
        return false;
    }
    h.retain(|x| x != &e);
    h.insert(0, e);
    h.truncate(MAX);
    true
}

fn add(history: &History, e: Entry) {
    let mut h = history.borrow_mut();
    if push(&mut h, e) {
        save(&h);
    }
}

fn store_image(p: &Pixbuf) -> Option<Entry> {
    let png = p.save_to_bufferv("png", &[]).map_err(|e| eprintln!("clippyboardy: encoding image failed: {e}")).ok()?;
    let mut hasher = DefaultHasher::new();
    png.hash(&mut hasher);
    let name = format!("{:016x}.png", hasher.finish());
    let path = data_dir().join(&name);
    if !path.exists() {
        ensure_dir()
            .and_then(|_| fs::write(&path, &png))
            .map_err(|e| eprintln!("clippyboardy: saving image failed: {e}"))
            .ok()?;
    }
    let t = thumb(&name);
    Some(Entry::Image(name, t))
}

fn record(history: &History) {
    let history = history.clone();
    // Text first: spreadsheets and editors also offer a picture of copied text.
    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).request_text(move |cb, text| match text {
        Some(t) => add(&history, Entry::Text(t.into())),
        None => cb.request_image(move |_, p| {
            if let Some(e) = p.and_then(store_image) {
                add(&history, e);
            }
        }),
    });
}

/// Virtual keyboard via /dev/uinput. It is kernel-level input, so GNOME shows no "remote interaction" prompt.
struct Keyboard(fs::File);

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const KEY_LEFTCTRL: u16 = 29;
const KEY_V: u16 = 47; // shortcut: physical key position, wrong on non-QWERTY layouts; use the keymap if needed
const UI_SET_EVBIT: libc::Ioctl = 0x4004_5564;
const UI_SET_KEYBIT: libc::Ioctl = 0x4004_5565;
const UI_DEV_SETUP: libc::Ioctl = 0x405c_5503;
const UI_DEV_CREATE: libc::Ioctl = 0x5501;

impl Keyboard {
    fn new() -> io::Result<Self> {
        let f = fs::OpenOptions::new().write(true).open("/dev/uinput")?;
        let fd = f.as_raw_fd();
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        setup.id = libc::input_id { bustype: 0x06 /* BUS_VIRTUAL */, vendor: 1, product: 1, version: 1 };
        for (dst, src) in setup.name.iter_mut().zip(b"clippyboardy") {
            *dst = *src as libc::c_char;
        }
        let ok = |r: libc::c_int| if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) };
        unsafe {
            ok(libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_int))?;
            ok(libc::ioctl(fd, UI_SET_KEYBIT, KEY_LEFTCTRL as libc::c_int))?;
            ok(libc::ioctl(fd, UI_SET_KEYBIT, KEY_V as libc::c_int))?;
            ok(libc::ioctl(fd, UI_DEV_SETUP, &setup))?;
            ok(libc::ioctl(fd, UI_DEV_CREATE))?;
        }
        Ok(Self(f))
    }

    fn ctrl_v(&self) -> io::Result<()> {
        for (code, value) in [(KEY_LEFTCTRL, 1), (KEY_V, 1), (KEY_V, 0), (KEY_LEFTCTRL, 0)] {
            self.emit(EV_KEY, code, value)?;
            self.emit(EV_SYN, 0, 0)?;
        }
        Ok(())
    }

    fn emit(&self, type_: u16, code: u16, value: i32) -> io::Result<()> {
        let ev = libc::input_event { time: libc::timeval { tv_sec: 0, tv_usec: 0 }, type_, code, value };
        let bytes = unsafe {
            std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<libc::input_event>())
        };
        (&self.0).write_all(bytes)
    }
}

const WIDTH: i32 = 520;
const VISIBLE_ROWS: i32 = 15;
const MIN_VISIBLE_ROWS: i32 = 10;
const FRAME_H: i32 = 6;

fn show(app: &gtk::Application, history: &History, keyboard: Option<Rc<Keyboard>>) {
    if let Some(w) = app.active_window() {
        w.close(); // rebuild so the list is fresh and the new window gets focus
    }
    let (app, history) = (app.clone(), history.clone());
    at_pointer(move |x, y| build(&app, &history, keyboard, x, y));
}

type PointerCallback = Rc<RefCell<Option<Box<dyn FnOnce(i32, i32)>>>>;

fn finish(overlay: &gtk::Window, cb: &PointerCallback, x: i32, y: i32) {
    overlay.close();
    if let Some(f) = cb.take() {
        f(x, y);
    }
}

/// Xwayland only learns the pointer position while it is over an X window, so briefly cover the
/// screen with an invisible one and read the position from the enter event.
fn at_pointer(f: impl FnOnce(i32, i32) + 'static) {
    let cb: PointerCallback = Rc::new(RefCell::new(Some(Box::new(f))));
    let last_known = || {
        gdk::Display::default()
            .and_then(|d| d.default_seat())
            .and_then(|s| s.pointer())
            .map(|p| { let (_, x, y) = p.position(); (x, y) })
            .unwrap_or((0, 0))
    };
    let screen = gdk::Screen::default();
    let (Some(visual), Some(root)) = (
        screen.as_ref().and_then(|s| s.rgba_visual()),
        screen.as_ref().and_then(|s| s.root_window()),
    ) else {
        let (x, y) = last_known(); // no transparency: skip the overlay rather than flash a black screen
        return cb.take().unwrap()(x, y);
    };
    let overlay = gtk::Window::new(gtk::WindowType::Popup);
    overlay.set_visual(Some(&visual));
    overlay.set_app_paintable(true);
    overlay.connect_draw(|_, cr| {
        cr.set_operator(gtk::cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        let _ = cr.paint();
        glib::Propagation::Stop
    });
    overlay.move_(0, 0);
    overlay.resize(root.width(), root.height());
    overlay.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::POINTER_MOTION_MASK);
    overlay.connect_enter_notify_event(glib::clone!(@strong cb => move |w, ev| {
        let (x, y) = ev.root();
        finish(w, &cb, x as i32, y as i32);
        glib::Propagation::Stop
    }));
    overlay.connect_motion_notify_event(glib::clone!(@strong cb => move |w, ev| {
        let (x, y) = ev.root();
        finish(w, &cb, x as i32, y as i32);
        glib::Propagation::Stop
    }));
    // Never leave an invisible window blocking clicks if no pointer event arrives.
    glib::timeout_add_local_once(Duration::from_millis(300), glib::clone!(@weak overlay => move || {
        let (x, y) = last_known();
        finish(&overlay, &cb, x, y);
    }));
    overlay.show();
}

/// A ▼ bar under the list: hovering scrolls down continuously, clicking jumps a page.
fn autoscroll_button(adj: &gtk::Adjustment) -> gtk::Button {
    let down = gtk::Button::with_label("▼");
    down.set_relief(gtk::ReliefStyle::None);
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Default::default();
    down.connect_enter_notify_event(glib::clone!(@strong timer, @weak adj => @default-return glib::Propagation::Proceed, move |_, _| {
        let id = glib::timeout_add_local(Duration::from_millis(16), glib::clone!(@weak adj => @default-return glib::ControlFlow::Break, move || {
            adj.set_value(adj.value() + 8.0);
            glib::ControlFlow::Continue
        }));
        if let Some(old) = timer.replace(Some(id)) {
            old.remove();
        }
        glib::Propagation::Proceed
    }));
    down.connect_leave_notify_event(move |_, _| {
        if let Some(id) = timer.take() {
            id.remove();
        }
        glib::Propagation::Proceed
    });
    down.connect_clicked(glib::clone!(@weak adj => move |_| adj.set_value(adj.value() + adj.page_size())));
    down
}

/// Window `(x, y)` and list height: below-right of the pointer (flipped left at the right edge).
/// When fewer than `min_h` pixels fit below the pointer, shifts up against the bottom of `area`
/// so the menu sits beside the pointer and still reveals `MIN_VISIBLE_ROWS` entries.
fn place(px: i32, py: i32, area: &gdk::Rectangle, desired_h: i32, min_h: i32, chrome_h: i32) -> (i32, i32, i32) {
    // +1 on the right mirrors `px - WIDTH` on the left so the pointer never starts on top of the ▼ button.
    let x = if px + 1 + WIDTH > area.x() + area.width() {
        (px - WIDTH).max(area.x())
    } else {
        (px + 1).max(area.x())
    };
    let top = area.y();
    let bottom = area.y() + area.height();
    let max_h = (area.height() - chrome_h).max(100);
    let min_h = min_h.min(max_h);
    let desired_h = desired_h.min(max_h).max(min_h);
    let start_y = py.max(top);
    let room = bottom - start_y - chrome_h;
    if room >= min_h {
        (x, start_y, desired_h.min(room))
    } else {
        let y = (bottom - min_h - chrome_h).max(top);
        (x, y, min_h)
    }
}

fn build(app: &gtk::Application, history: &History, keyboard: Option<Rc<Keyboard>>, px: i32, py: i32) {
    let entries = history.borrow().clone();
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Clipboard");
    win.set_decorated(false);
    win.set_skip_taskbar_hint(true);
    win.set_keep_above(true);
    let list = gtk::ListBox::new();
    let entries = Rc::new(RefCell::new(entries)); // shrinks with the list so row index == entry index
    for e in entries.borrow().iter() {
        let content: gtk::Widget = match e {
            Entry::Text(t) => {
                let preview: String = t.chars().take(300).collect::<String>().replace('\n', " ⏎ ");
                let label = gtk::Label::new(Some(&preview));
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_tooltip_text(Some(&t.chars().take(2000).collect::<String>()));
                label.upcast()
            }
            Entry::Image(_, Some(p)) => {
                let img = gtk::Image::from_pixbuf(Some(p));
                img.set_halign(gtk::Align::Start);
                img.upcast()
            }
            Entry::Image(_, None) => gtk::Label::new(Some("[image]")).upcast(),
        };
        content.set_hexpand(true); // takes the leftover width; the ellipsized label never runs under the bin
        let bin = gtk::Button::from_icon_name(Some("user-trash-symbolic"), gtk::IconSize::Button);
        bin.set_relief(gtk::ReliefStyle::None);
        bin.set_valign(gtk::Align::Center);
        bin.set_tooltip_text(Some("Remove"));
        bin.connect_clicked(glib::clone!(@weak list, @strong entries, @strong history => move |b| {
            let Some(row) = b.ancestor(gtk::ListBoxRow::static_type()).and_then(|w| w.downcast::<gtk::ListBoxRow>().ok()) else { return };
            let e = entries.borrow_mut().remove(row.index() as usize);
            let mut h = history.borrow_mut();
            h.retain(|x| x != &e);
            save(&h); // also deletes the image file
            list.remove(&row); // window keeps its size so the next bin stays under the cursor
        }));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add(&content);
        row.add(&bin);
        row.set_margin_top(6);
        row.set_margin_bottom(6);
        row.set_margin_start(8);
        row.set_margin_end(8);
        list.add(&row);
    }
    if entries.borrow().is_empty() {
        list.add(&gtk::Label::new(Some("Clipboard history is empty")));
    }
    list.connect_row_activated(glib::clone!(@weak win, @strong history => move |_, row| {
        let Some(e) = entries.borrow().get(row.index() as usize).cloned() else { return };
        let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        match &e {
            Entry::Text(t) => cb.set_text(t),
            Entry::Image(f, _) => match Pixbuf::from_file(data_dir().join(f)) {
                Ok(p) => cb.set_image(&p),
                Err(err) => return eprintln!("clippyboardy: loading image failed: {err}"),
            },
        }
        add(&history, e); // now entry 1 and what Ctrl+V pastes
        win.close();
        if let Some(kb) = keyboard.clone() {
            // Give GNOME a moment to hand focus back to the previous window before pressing Ctrl+V.
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                if let Err(e) = kb.ctrl_v() {
                    eprintln!("clippyboardy: auto-paste failed: {e}");
                }
            });
        }
    }));
    win.connect_key_press_event(|w, ev| {
        if ev.keyval() == gdk::keys::constants::Escape {
            w.close();
        }
        glib::Propagation::Proceed
    });
    // GNOME sometimes refuses focus, or hands it back to the previous app, right after we open.
    // For the first 0.7s keep asking for it; after that, losing focus means the user clicked away.
    let opened = std::time::Instant::now();
    let grace = Duration::from_millis(700);
    glib::timeout_add_local(Duration::from_millis(100), glib::clone!(@weak win => @default-return glib::ControlFlow::Break, move || {
        if !win.is_active() {
            focus_now(&win);
        }
        if opened.elapsed() < grace { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
    }));
    win.connect_focus_out_event(move |w, _| {
        let w = w.clone();
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if !w.is_active() && opened.elapsed() >= grace {
                w.close();
            }
        });
        glib::Propagation::Proceed
    });

    list.show_all();
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_propagate_natural_height(true);
    let adj = scroll.vadjustment();
    list.set_focus_vadjustment(&adj); // keyboard navigation keeps the selected row in view
    scroll.add(&list);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add(&scroll);

    let area = gdk::Display::default()
        .and_then(|d| d.monitor_at_point(px, py))
        .map(|m| m.workarea())
        .unwrap_or_else(|| gdk::Rectangle::new(0, 0, i32::MAX / 4, i32::MAX / 4));
    let sum_rows = |n: usize| -> i32 {
        (0..n).filter_map(|i| list.row_at_index(i as i32)).map(|r| r.preferred_height().1).sum()
    };
    let raw_all_h = sum_rows(list.children().len());
    let list_all_h = list.preferred_height().1.max(raw_all_h);
    let extra = list_all_h - raw_all_h;
    let desired_h = (sum_rows(VISIBLE_ROWS as usize) + extra).min(list_all_h);
    let min_h = (sum_rows(MIN_VISIBLE_ROWS as usize) + extra).min(list_all_h);
    let (mut x, mut y, mut list_h) = place(px, py, &area, desired_h, min_h, FRAME_H);
    if list_all_h > list_h {
        let down = autoscroll_button(&adj);
        down.show_all(); // hidden widgets measure 0 tall
        let button_h = down.preferred_height().1;
        vbox.add(&down);
        (x, y, list_h) = place(px, py, &area, desired_h, min_h, FRAME_H + button_h);
    }
    scroll.set_max_content_height(list_h);

    let frame = gtk::Frame::new(None);
    frame.add(&vbox);
    win.add(&frame);
    win.set_default_size(WIDTH, -1);
    win.move_(x, y);
    frame.show_all();
    win.realize(); // so the user time is set before GNOME sees the window map
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
    focus_now(&win);
}

extern "C" {
    // In libgdk-3, already linked.
    fn gdk_x11_get_server_time(window: *mut gdk::ffi::GdkWindow) -> u32;
    fn gdk_x11_window_set_user_time(window: *mut gdk::ffi::GdkWindow, timestamp: u32);
}

/// Shows and focuses `win` with a fresh timestamp. GNOME's focus-stealing prevention refuses windows whose
/// user time is older than the focused app's, and GTK's default is our last key/click, possibly minutes old.
fn focus_now(win: &gtk::ApplicationWindow) {
    let Some(gw) = win.window() else { return win.present() };
    let ptr: *mut gdk::ffi::GdkWindow = glib::translate::ToGlibPtr::to_glib_none(&gw).0;
    let now = unsafe { gdk_x11_get_server_time(ptr) };
    unsafe { gdk_x11_window_set_user_time(ptr, now) };
    win.present_with_time(now);
}

fn main() -> glib::ExitCode {
    // GNOME has no data-control protocol; via Xwayland we can watch the clipboard unfocused, without portal prompts.
    gdk::set_allowed_backends("x11");
    let daemon = std::env::args().any(|a| a == "--daemon");
    let app = gtk::Application::new(Some("dev.clippyboardy"), Default::default());
    let history: History = Default::default();
    let skip_first = std::cell::Cell::new(daemon);

    app.connect_startup(glib::clone!(@strong history => move |app| {
        std::mem::forget(app.hold()); // keep running with no window: we must stay alive to own pasted entries
        *history.borrow_mut() = load();
        record(&history);
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).connect_local(
            "owner-change",
            false,
            glib::clone!(@strong history => move |_| {
                record(&history);
                None
            }),
        );
    }));
    if let Err(e) = app.register(gio::Cancellable::NONE) {
        eprintln!("clippyboardy: {e}");
        return glib::ExitCode::FAILURE;
    }
    // Only the long-running instance needs the keyboard; created early so GNOME has registered it before first use.
    let keyboard = if app.is_remote() {
        None
    } else {
        Keyboard::new()
            .map_err(|e| eprintln!("clippyboardy: auto-paste disabled, cannot use /dev/uinput: {e}"))
            .ok()
            .map(Rc::new)
    };
    app.connect_activate(move |app| {
        if !skip_first.replace(false) {
            show(app, &history, keyboard.clone());
        }
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_dedupes_and_caps() {
        let t = |s: &str| Entry::Text(s.into());
        let mut h = Vec::new();
        for i in 0..40 {
            assert!(push(&mut h, t(&i.to_string())));
        }
        assert_eq!(h.len(), MAX);
        assert!(h[0] == t("39"));
        assert!(!push(&mut h, t("39")), "same as top is a no-op");
        assert!(!push(&mut h, t("  \n")), "blank ignored");
        assert!(push(&mut h, t("20")));
        assert!(h[0] == t("20") && h.len() == MAX);
        assert_eq!(h.iter().filter(|e| **e == t("20")).count(), 1);
        assert!(push(&mut h, Entry::Image("20".into(), None)), "image never equals text");
    }

    #[test]
    fn place_stays_mouse_relative_and_shows_min_near_bottom() {
        let area = gdk::Rectangle::new(1920, 0, 1920, 1032); // right monitor, dock at bottom
        let (desired, min, chrome) = (600, 400, 40);
        assert_eq!(
            place(2000, 100, &area, desired, min, chrome),
            (2001, 100, 600),
            "plenty of room: below-right of pointer with full desired height"
        );
        assert_eq!(
            place(2000, 500, &area, desired, min, chrome),
            (2001, 500, 492),
            "between desired and min room: stays at pointer and trims to fit"
        );
        assert_eq!(
            place(2000, 900, &area, desired, min, chrome),
            (2001, 1032 - 400 - 40, 400),
            "too low on screen: shifts up beside right of pointer showing min height (10 entries)"
        );
        assert_eq!(
            place(3700, 100, &area, desired, min, chrome),
            (3700 - WIDTH, 100, 600),
            "near right edge: left of pointer"
        );
        assert_eq!(
            place(3700, 950, &area, desired, min, chrome),
            (3700 - WIDTH, 1032 - 400 - 40, 400),
            "bottom-right corner: left of pointer and shifted up showing 10 entries"
        );
        assert_eq!(
            place(2000, 950, &area, 180, 180, 6),
            (2001, 1032 - 180 - 6, 180),
            "small history near bottom: shifts up just enough to reveal all entries"
        );
        let with_panel = gdk::Rectangle::new(0, 32, 1920, 1000);
        assert_eq!(
            place(200, 10, &with_panel, desired, min, chrome),
            (201, 32, 600),
            "pointer on top bar: clamps below top of workarea"
        );
        assert_eq!(
            place(200, 100, &with_panel, 1400, 1200, chrome),
            (201, 32, 960),
            "giant entries taller than screen: capped to workarea height"
        );
    }

    #[test]
    #[ignore = "creates a real virtual keyboard device"]
    fn uinput_keyboard_creates() {
        Keyboard::new().unwrap();
    }
}
