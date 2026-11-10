use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) const FPS: f64 = 10.0;

#[derive(Default)]
struct Extract {
    length: Option<f64>,
    frames: Option<usize>,
    error: Option<String>,
}

pub(crate) struct Video {
    dir: PathBuf,
    extract: Arc<Mutex<Extract>>,
    at: f64,
    since: Option<Instant>,
    pub(crate) shown: Option<usize>,
}

pub(crate) fn frame_at(seconds: f64, frames: Option<usize>) -> usize {
    let wanted = (seconds.max(0.0) * FPS) as usize;
    frames.map_or(wanted, |count| wanted.min(count.saturating_sub(1)))
}

pub(crate) fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

impl Video {
    pub(crate) fn start(path: &Path) -> Video {
        let dir = std::env::temp_dir().join(format!("revui-frames-{}", crate::uuid()));
        let extract = Arc::new(Mutex::new(Extract::default()));
        let (source, frames, shared) = (path.to_path_buf(), dir.clone(), extract.clone());
        std::thread::spawn(move || {
            let probe = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(&source).output();
            let Ok(probe) = probe else {
                shared.lock().unwrap().error = Some("ffmpeg is needed to play it here.".into());
                return;
            };
            shared.lock().unwrap().length = String::from_utf8_lossy(&probe.stdout).trim().parse().ok();
            if Arc::strong_count(&shared) == 1 {
                return;
            }
            let _ = std::fs::create_dir_all(&frames);
            let filter = format!("fps={FPS},scale='min(1200,iw)':-2");
            let run = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&source).args(["-vf", &filter, "-q:v", "3"]).arg(frames.join("%05d.jpg")).output();
            if Arc::strong_count(&shared) == 1 {
                let _ = std::fs::remove_dir_all(&frames);
                return;
            }
            let count = std::fs::read_dir(&frames).map_or(0, |entries| entries.count());
            let mut done = shared.lock().unwrap();
            match (run, count) {
                (Ok(_), 1..) => done.frames = Some(count),
                _ => done.error = Some("ffmpeg could not read it.".into()),
            }
        });
        Video { dir, extract, at: 0.0, since: None, shown: None }
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.extract.lock().unwrap().error.clone()
    }

    pub(crate) fn length(&self) -> Option<f64> {
        let extract = self.extract.lock().unwrap();
        extract.length.or(extract.frames.map(|count| count as f64 / FPS))
    }

    pub(crate) fn playing(&self) -> bool {
        self.since.is_some()
    }

    pub(crate) fn position(&mut self) -> f64 {
        let Some(since) = self.since else { return self.at };
        let now = self.at + since.elapsed().as_secs_f64();
        match self.length() {
            Some(length) if now >= length => {
                (self.at, self.since) = (length, None);
                length
            }
            _ => now,
        }
    }

    pub(crate) fn toggle(&mut self) {
        self.at = self.position();
        self.since = match self.since {
            Some(_) => None,
            None => {
                if self.length().is_some_and(|length| self.at >= length) {
                    self.at = 0.0;
                }
                Some(Instant::now())
            }
        };
    }

    pub(crate) fn pause(&mut self) {
        if self.playing() {
            self.toggle();
        }
    }

    pub(crate) fn frame(&mut self) -> Option<(usize, PathBuf)> {
        let frames = self.extract.lock().unwrap().frames;
        let wanted = frame_at(self.position(), frames);
        let file = self.dir.join(format!("{:05}.jpg", wanted + 1));
        let ready = frames.is_some() || self.dir.join(format!("{:05}.jpg", wanted + 2)).exists();
        (self.shown != Some(wanted) && ready && file.exists()).then_some((wanted, file))
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
