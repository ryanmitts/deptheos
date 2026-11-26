//! One render's folder read without the Library: no `History` and no
//! `library.lock`, so it reads while an app holds the Library (the redraw
//! command, ADR-103). It writes nothing but what SQLite keeps beside the
//! paint to share it with a writer, its `-wal` and `-shm`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fsutil::read_versioned;
use crate::library::render_file::{RENDER_FILE, RenderFile, load_stack};
use crate::runs::session::CompletedStack;
use crate::{Error, Result};

pub struct RenderFolder {
    pub stack: Arc<CompletedStack>,
    /// The stack's pixels with its paint flattened over them, when it has
    /// any: what the app shows.
    pub painted: Option<Vec<u16>>,
}

impl RenderFolder {
    /// The render in `dir`. Its frames are named by their paths within
    /// their folders, not looked up on disk, so an export carries no
    /// frame's camera metadata.
    pub fn read(dir: &Path) -> Result<Self> {
        let file: RenderFile = read_versioned(&dir.join(RENDER_FILE))?;
        file.check()?;
        let listed: Vec<PathBuf> = file.frames.iter().map(|f| PathBuf::from(&f.path)).collect();
        let paths = file.included_paths(&listed);
        let stack = Arc::new(load_stack(dir, file, paths)?);
        let doc = crate::paint::store::load_doc(dir, stack.width, stack.height)
            .map_err(Error::PaintUnread)?;
        let painted = doc.filter(|d| d.is_painted()).and_then(|mut doc| {
            doc.open(stack.clone());
            doc.canvas().map(|c| c.flatten())
        });
        Ok(Self { stack, painted })
    }

    /// Whether `dir` holds a render.
    pub fn is_render(dir: &Path) -> bool {
        dir.join(RENDER_FILE).is_file()
    }

    /// What the app shows: the painted pixels, else the stack's own.
    pub fn pixels(&self) -> &[u16] {
        self.painted.as_deref().unwrap_or(&self.stack.pixels)
    }
}

#[cfg(test)]
mod tests {
    use image::RgbImage;

    use super::*;
    use crate::library::history::History;
    use crate::library::ids::StackId;

    /// A render and its paint read while the Library is held, adding
    /// nothing to the folder but SQLite's files beside the paint.
    #[test]
    fn a_render_reads_with_its_paint_while_the_library_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        let frames: Vec<PathBuf> = ["a.png", "b.png"].map(|n| dir.path().join(n)).to_vec();
        for frame in &frames {
            std::fs::write(frame, b"frame").unwrap();
        }
        let (w, h) = (64, 48);
        let pixels = (0..w * h * 3).map(|i| (i * 13 % 65_536) as u16).collect();
        let stack = CompletedStack::srgb(w, h, pixels, frames.clone());
        let thumb = RgbImage::new(w, h);
        let render = history.save_render(&frames, &stack, &thumb, &StackId::new(), None).unwrap();
        let folder = history.render_path(&render.id);

        let unpainted = RenderFolder::read(&folder).unwrap();
        assert_eq!(unpainted.stack.pixels, stack.pixels);
        assert_eq!(
            unpainted.stack.paths,
            frames
                .iter()
                .map(|p| p.strip_prefix(dir.path()).unwrap().to_path_buf())
                .collect::<Vec<_>>()
        );
        assert!(unpainted.painted.is_none());

        let mut doc = crate::paint::store::open_doc(&folder, w, h);
        doc.open(unpainted.stack.clone());
        crate::paint::undo::tests::paint(&mut doc, 0);
        let flat = doc.canvas().unwrap().flatten();
        assert!(crate::paint::store::save(&doc).unwrap());
        drop(doc);

        let files = || -> Vec<String> {
            let names = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name());
            let mut names: Vec<String> = names.map(|n| n.to_string_lossy().into_owned()).collect();
            names.retain(|n| !n.ends_with("-wal") && !n.ends_with("-shm"));
            names.sort();
            names
        };
        let before = files();
        let painted = RenderFolder::read(&folder).unwrap();
        assert_eq!(painted.painted.as_deref(), Some(flat.as_slice()));
        assert_ne!(painted.pixels(), stack.pixels.as_slice());
        assert_eq!(before, files());
    }
}
