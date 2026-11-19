//! `DATABUG_SHOT_SCENE=demo databug --screenshot <folder>`: plays a scripted
//! tour (SQL with completion, Redis, CouchDB documents, the engine list) and
//! saves every frame as a JPEG in the folder, for turning into a video:
//!
//! ```sh
//! ffmpeg -framerate 30 -i <folder>/f%05d.jpg -vf scale=1440:-2 -c:v libx264 \
//!   -pix_fmt yuv420p -crf 22 -movflags +faststart demo.mp4
//! ```
//!
//! `DATABUG_HOME` should hold connections named "Local Redis" (a Redis with
//! the sample keys) and "Local CouchDB" (a `store` database with
//! `photo:0001`); `DATABUG_SHOT_PASSWORDS="Local Redis=…;Local CouchDB=…"`
//! gives their passwords for this run only. The frame number is the clock,
//! so the video plays at an even speed however fast frames are captured.

use super::*;

/// Video frames per second the timeline is written for.
pub const FPS: u32 = 30;
/// Frames between typed characters (about 15 characters a second), plus a
/// little after spaces and punctuation, like a person typing.
const FRAMES_PER_CHAR: u32 = 2;

enum Step {
    /// Type text one character at a time.
    Type(&'static str),
    Key(Key, Modifiers),
    /// Do nothing for this many frames.
    Hold(u32),
    Do(fn(&mut DataBugApp, &egui::Context)),
}

pub struct Demo {
    dir: std::path::PathBuf,
    frame: u32,
    pending: bool,
    steps: Vec<Step>,
    /// Index of the current step, and progress within it (characters typed
    /// or frames held).
    at: usize,
    progress: usize,
    /// Frames left before the next typed character.
    wait: u32,
    /// Escape was sent for the newline being typed; Enter follows next frame.
    escaped: bool,
    tx: Option<std::sync::mpsc::SyncSender<(u32, egui::ColorImage)>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

fn conn_named(app: &DataBugApp, name: &str) -> Option<String> {
    app.connections.iter().find(|c| c.name == name).map(|c| c.id.clone())
}

fn open_named(app: &mut DataBugApp, ctx: &egui::Context, name: &str) {
    let Some(id) = conn_named(app, name) else {
        eprintln!("demo: no connection named {name}");
        return;
    };
    app.explorer.open_key(format!("c:{id}"));
    app.open_console(&id, Some(String::new()), ctx);
}

fn script() -> Vec<Step> {
    use Step::*;
    let enter = || Key(egui::Key::Enter, Modifiers::NONE);
    let run = || Key(egui::Key::Enter, Modifiers::COMMAND);
    vec![
        Do(|app, ctx| {
            // passwords for this run only, kept in memory
            for pair in std::env::var("DATABUG_SHOT_PASSWORDS").unwrap_or_default().split(';') {
                if let Some((name, pw)) = pair.split_once('=') {
                    if let Some(id) = conn_named(app, name) {
                        app.store.set_password(&id, pw, false);
                    }
                }
            }
            app.open_sample(ctx);
            if let Some(t) = app.tabs.get_mut(app.active) {
                t.editor.set_text(String::new());
                t.editor.focus();
            }
        }),
        Hold(36),
        // SQL with schema-aware completion
        Type("-- Top customers by spend\nSELECT c.name, SUM(o.total) AS spent\nFROM cu"),
        Hold(20),
        enter(),
        Type(" c\nJOIN orders o ON o.customer_id = c.id\nGROUP BY c.name\nORDER BY spent DESC;"),
        Hold(8),
        run(),
        Hold(84),
        // Redis: keys in the explorer, commands with key completion
        Do(|app, ctx| {
            open_named(app, ctx, "Local Redis");
            if let Some(id) = conn_named(app, "Local Redis") {
                app.explorer.open_key(format!("g:{id}:db0:Key"));
            }
        }),
        Hold(32),
        Type("# Redis: a user, then the leaderboard\nHGETALL us"),
        Hold(24),
        enter(),
        Hold(6),
        run(),
        Hold(60),
        Type("\nZREVRANGE lea"),
        Hold(20),
        enter(),
        Type(" 0 4 WITHSCORES"),
        Hold(6),
        run(),
        Hold(72),
        // CouchDB: documents, their metadata and attachments
        Do(|app, ctx| open_named(app, ctx, "Local CouchDB")),
        Hold(28),
        Type("// CouchDB documents and attachments\ndb.store.find({type: \"photo\"})"),
        Hold(6),
        run(),
        Hold(132),
        // operator completion with documentation
        Type("\ndb.store.find({rating: {$g"),
        Hold(52),
        Key(egui::Key::Escape, Modifiers::NONE),
        Type("te: 4}})"),
        Hold(6),
        run(),
        Hold(60),
        // every engine
        Do(|app, _| {
            let mut d = ConnDialog::new(ConnectionConfig::new(DbKind::Redis), String::new(), true);
            d.open_engines = true;
            app.dialog = Some(d);
        }),
        Hold(100),
        Do(|app, _| app.dialog = None),
        // light theme
        Do(|app, _| app.settings.theme = ThemeKind::Light),
        Hold(72),
        Do(|app, _| app.settings.theme = ThemeKind::Dark),
        Hold(24),
    ]
}

impl Demo {
    pub fn new(dir: std::path::PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        // encode on a few threads; the bounded queue paces capture
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u32, egui::ColorImage)>(4);
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let workers = (0..4)
            .map(|_| {
                let rx = rx.clone();
                let dir = dir.clone();
                std::thread::spawn(move || loop {
                    let job = rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some((n, img)) = job else { break };
                    if let Err(e) = save_jpeg(&dir.join(format!("f{n:05}.jpg")), &img) {
                        eprintln!("demo: frame {n}: {e}");
                    }
                })
            })
            .collect();
        Demo { dir, frame: 0, pending: false, steps: script(), at: 0, progress: 0, wait: 0, escaped: false, tx: Some(tx), workers }
    }

    /// Advance the script by one frame. Returns false when it's done.
    fn step(&mut self, app: &mut DataBugApp, ctx: &egui::Context) -> bool {
        loop {
            let Some(s) = self.steps.get(self.at) else { return false };
            match s {
                Step::Type(text) => {
                    if self.wait > 0 {
                        self.wait -= 1;
                        return true;
                    }
                    let chars: Vec<char> = text.chars().collect();
                    let c = chars[self.progress];
                    let key = |k: Key| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
                    if c == '\n' && !self.escaped {
                        // close any completion popup first (a frame earlier), so
                        // Enter starts a new line instead of accepting a suggestion
                        ctx.input_mut(|inp| inp.events.push(key(Key::Escape)));
                        self.escaped = true;
                        return true;
                    }
                    if c == '\n' {
                        self.escaped = false;
                        ctx.input_mut(|inp| inp.events.push(key(Key::Enter)));
                    } else {
                        ctx.input_mut(|inp| inp.events.push(egui::Event::Text(c.to_string())));
                    }
                    // a steady rhythm with small, repeatable variations
                    let jitter = (self.progress as u32 * 7 + self.at as u32 * 3) % 3;
                    self.wait = FRAMES_PER_CHAR - 1 + jitter / 2
                        + match c {
                            ' ' => 1,
                            ',' | '.' | ':' | ';' | '(' | ')' | '{' | '}' => 2,
                            '\n' => 4,
                            _ => 0,
                        };
                    self.progress += 1;
                    if self.progress >= chars.len() {
                        self.at += 1;
                        self.progress = 0;
                        self.wait = 0;
                    }
                    return true;
                }
                Step::Key(k, m) => {
                    let (k, m) = (*k, *m);
                    ctx.input_mut(|inp| inp.events.push(egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }));
                    self.at += 1;
                    return true;
                }
                Step::Hold(n) => {
                    self.progress += 1;
                    if self.progress >= *n as usize {
                        self.at += 1;
                        self.progress = 0;
                    }
                    return true;
                }
                Step::Do(f) => {
                    let f = *f;
                    f(app, ctx);
                    self.at += 1;
                }
            }
        }
    }

    fn finish(&mut self) {
        self.tx = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        eprintln!("demo: {} frames ({:.1} s at {FPS} fps) in {}", self.frame, self.frame as f32 / FPS as f32, self.dir.display());
    }
}

impl DataBugApp {
    /// One frame of the demo: save the last capture, advance the script and
    /// ask for the next capture.
    pub(super) fn drive_demo(&mut self, ctx: &egui::Context) {
        let Some(mut demo) = self.demo.take() else { return };
        ctx.request_repaint();
        if demo.pending {
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            match shot {
                Some(img) => {
                    if let Some(tx) = &demo.tx {
                        let _ = tx.send((demo.frame, (*img).clone()));
                    }
                    demo.frame += 1;
                    demo.pending = false;
                }
                None => {
                    self.demo = Some(demo);
                    return;
                }
            }
        }
        if !demo.step(self, ctx) {
            demo.finish();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        demo.pending = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        self.demo = Some(demo);
    }
}

fn save_jpeg(path: &std::path::Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    let [w, h] = img.size;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in &img.pixels {
        rgb.extend_from_slice(&[p.r(), p.g(), p.b()]);
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(f, 92).encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)?;
    Ok(())
}
