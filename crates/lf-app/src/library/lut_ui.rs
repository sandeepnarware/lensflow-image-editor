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

    /// Develop's "Import .dcp…": the file picker, on its own thread.
    pub(super) fn pick_dcp(&mut self, env: &mut Env) {
        on_own_thread(env.jobs.reporter(), Purpose::DcpFile, 0, move || {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a .dcp camera profile")
                .add_filter("DNG camera profiles", &["dcp"])
                .pick_file();
            log::info!("camera profile picker answered: {picked:?}");
            Ok(picked.map(|path| Chosen {
                path,
                network: false,
            }))
        });
    }

    /// Copies a chosen `.dcp` into the catalogue's profile folder and loads it (`Io` lane).
    pub fn import_dcp(&mut self, file: PathBuf, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "import camera profile", move || {
            Msg::DcpImported(crate::profiles::import(&root, &file))
        });
    }

    /// View → Proof profile… (or Soft proof with none chosen yet): the ICC file picker.
    pub(super) fn pick_proof_profile(&mut self, env: &mut Env) {
        on_own_thread(env.jobs.reporter(), Purpose::ProofProfile, 0, move || {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a printer and paper profile")
                .add_filter("ICC profiles", &["icc", "icm"])
                .pick_file();
            log::info!("proof profile picker answered: {picked:?}");
            Ok(picked.map(|path| Chosen {
                path,
                network: false,
            }))
        });
    }

    /// S: soft-proof on or off; the first time, a profile is asked for.
    pub(super) fn toggle_proof(&mut self, env: &mut Env) {
        if self.develop.proofing() {
            self.develop.set_proof(None);
        } else if let Some(p) = env.prefs.proof_profile.clone().filter(|p| p.exists()) {
            self.develop.set_proof(Some(p));
        } else {
            self.pick_proof_profile(env);
        }
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
