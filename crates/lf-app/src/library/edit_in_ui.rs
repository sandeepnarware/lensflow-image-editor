//! The library's side of "Edit in external editor": choosing the editor once (an application
//! picker on its own thread), rendering the copy and opening it on the `Long` lane
//! ([`crate::edit_in`]), and, when it is made, rescanning the catalogue folder it lies in so
//! the copy shows beside the original.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library, Mode};
use crate::jobs::{Lane, Msg};
use crate::toast::Level;
use std::path::PathBuf;

/// No editor is chosen (or the chosen one is gone): the picker asks for one.
pub const CHOOSE_EDITOR: &str = "Choose the application to edit photos in";

impl Library {
    /// The photo "Edit in" works on: the one open in Develop, else the active one.
    fn edit_in_target(&self) -> Option<i64> {
        match self.mode {
            Mode::Develop(id) => Some(id),
            Mode::Library => self.selection.active(),
        }
    }

    /// `Ctrl+E`: with an editor chosen (and still there), the copy is made and opened;
    /// otherwise the application picker asks first.
    pub(super) fn edit_in(&mut self, env: &mut Env) {
        match env.prefs.external_editor.clone().filter(|p| p.exists()) {
            Some(editor) => self.start_edit_in(editor, env),
            None => {
                on_own_thread(env.jobs.reporter(), Purpose::Editor, 0, move || {
                    let mut dialog = rfd::FileDialog::new().set_title(CHOOSE_EDITOR);
                    if cfg!(windows) {
                        dialog = dialog.add_filter("Applications", &["exe"]);
                    }
                    let picked = if cfg!(target_os = "macos") {
                        // An application is a folder (`.app`) on macOS.
                        dialog.pick_folder()
                    } else {
                        dialog.pick_file()
                    };
                    log::info!("editor picker answered: {picked:?}");
                    Ok(picked.map(|path| Chosen {
                        path,
                        network: false,
                    }))
                });
            }
        }
    }

    /// The editor was chosen: kept, and the copy made.
    pub(super) fn editor_chosen(&mut self, editor: PathBuf, env: &mut Env) {
        env.prefs.external_editor = Some(editor.clone());
        self.start_edit_in(editor, env);
    }

    fn start_edit_in(&mut self, editor: PathBuf, env: &mut Env) {
        let (Some(id), Some(root)) = (self.edit_in_target(), self.root.clone()) else {
            return;
        };
        // The open photo's gesture is recorded first, so the copy carries it.
        self.develop.finish_edit(env.toasts);
        let gpu = self.gpu.clone();
        env.toasts
            .push(Level::Info, "Making a copy for the editor…");
        env.jobs.spawn(Lane::Long, "edit in", move || {
            let result = crate::edit_in::render_copy(gpu, &root, id).map(|copy| {
                let failed = crate::platform::open_with(&editor, &copy).err().map(|e| {
                    format!(
                        "The copy was made, but {} could not be started ({e}).",
                        editor.display()
                    )
                });
                (copy, failed)
            });
            Msg::EditInDone(result)
        });
    }

    /// The copy was made (and opened, or why not), or why it was not; a copy's folder is
    /// rescanned to catalogue it.
    pub(super) fn edit_in_done(
        &mut self,
        result: Result<(PathBuf, Option<String>), String>,
        env: &mut Env,
    ) {
        match result {
            Ok((copy, failed)) => {
                let name = copy
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match failed {
                    None => env.toasts.push(
                        Level::Success,
                        format!(
                            "{name} is open in the editor. Rescan the folder after saving it there."
                        ),
                    ),
                    Some(why) => env.toasts.push(Level::Warning, format!("{name}: {why}")),
                }
                let folder = self
                    .folders
                    .iter()
                    .find(|f| copy.starts_with(&f.path))
                    .cloned();
                if let Some(folder) = folder {
                    self.rescan(&folder, env);
                }
            }
            Err(e) => env.toasts.push(Level::Error, e),
        }
    }
}
