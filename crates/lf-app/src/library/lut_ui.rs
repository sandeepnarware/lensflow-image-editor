//! Develop's "Import .cube…" on the library's side: the file picker (its own thread, as
//! the folder pickers) and the copy into the catalogue's LUT folder on the `Io` lane
//! ([`crate::luts::import`]); the answer puts the LUT on the open photo.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::jobs::{Lane, Msg};
use std::path::PathBuf;

impl Library {
    /// Develop's "Import .cube…": the file picker, on its own thread.
    pub(super) fn pick_lut(&mut self, env: &mut Env) {
        on_own_thread(env.jobs.reporter(), Purpose::LutFile, 0, move || {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a .cube LUT")
                .add_filter("3D LUT", &["cube"])
                .pick_file();
            log::info!("LUT picker answered: {picked:?}");
            Ok(picked.map(|path| Chosen {
                path,
                network: false,
            }))
        });
    }

    /// Copies a chosen `.cube` into the catalogue's LUT folder and loads it (`Io` lane).
    pub fn import_lut(&mut self, file: PathBuf, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "import LUT", move || {
            Msg::LutImported(crate::luts::import(&root, &file))
        });
    }
}
