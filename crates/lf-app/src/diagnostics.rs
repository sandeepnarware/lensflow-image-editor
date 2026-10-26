//! The facts a support request needs (Help -> About -> Copy diagnostics): the version and
//! target, the operating system, the graphics adapter, the log folder, the catalogue and
//! the server. Nothing secret (no token, no email).

use crate::logging;
use crate::prefs::{Dirs, InstanceChoice, Prefs};
use lf_pipeline::Gpu;

/// What the graphics adapter is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuFacts {
    pub name: String,
    pub backend: String,
    pub software: bool,
    pub max_texture: u32,
}

impl GpuFacts {
    pub fn of(gpu: &Gpu) -> GpuFacts {
        let info = gpu.adapter.get_info();
        GpuFacts {
            name: info.name,
            backend: format!("{:?}", info.backend),
            software: gpu.is_software(),
            max_texture: gpu.max_edge(),
        }
    }
}

/// The diagnostics text: one fact per line. `gpu` is the adapter, or why there is none.
pub fn text(gpu: Result<&GpuFacts, &str>, dirs: &Dirs, prefs: &Prefs) -> String {
    let graphics = match gpu {
        Ok(g) => format!(
            "Graphics: {} ({}), software renderer: {}, max texture size: {} px",
            g.name,
            g.backend,
            if g.software { "yes" } else { "no" },
            g.max_texture
        ),
        Err(why) => format!("Graphics: none ({why})"),
    };
    let catalogue = prefs
        .catalogue_root
        .as_ref()
        .map_or_else(|| "not chosen yet".to_owned(), |p| p.display().to_string());
    let server = match prefs.instance {
        InstanceChoice::Prod => "lensflow.in",
        InstanceChoice::Dev => "development (dev.lensflow.in)",
    };
    [
        crate::cli::version_text(),
        format!("OS: {} ({})", std::env::consts::OS, std::env::consts::ARCH),
        graphics,
        format!("Log folder: {}", logging::dir(&dirs.data).display()),
        format!("Catalogue: {catalogue}"),
        format!("Server: {server}"),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::InstanceChoice;
    use std::path::PathBuf;

    fn dirs() -> Dirs {
        Dirs {
            config: PathBuf::from("/cfg"),
            data: PathBuf::from("/data"),
            cache: PathBuf::from("/cache"),
        }
    }

    #[test]
    fn diagnostics_names_the_log_folder_and_adapter() {
        let gpu = GpuFacts {
            name: "Microsoft Basic Render Driver".into(),
            backend: "Dx12".into(),
            software: true,
            max_texture: 16_384,
        };
        let prefs = Prefs {
            catalogue_root: Some(PathBuf::from("/photos")),
            instance: InstanceChoice::Dev,
            ..Prefs::default()
        };
        let text = text(Ok(&gpu), &dirs(), &prefs);
        assert!(text.starts_with(&crate::cli::version_text()), "{text}");
        assert!(text.contains(std::env::consts::OS), "{text}");
        assert!(
            text.contains("Microsoft Basic Render Driver (Dx12)"),
            "{text}"
        );
        assert!(text.contains("software renderer: yes"), "{text}");
        assert!(text.contains("16384 px"), "{text}");
        let logs = crate::logging::dir(&dirs().data);
        assert!(
            text.contains(&format!("Log folder: {}", logs.display())),
            "{text}"
        );
        assert!(text.contains(&format!(
            "Catalogue: {}",
            PathBuf::from("/photos").display()
        )));
        assert!(
            text.contains("Server: development (dev.lensflow.in)"),
            "{text}"
        );

        let text = super::text(Err("no adapter"), &dirs(), &Prefs::default());
        assert!(text.contains("Graphics: none (no adapter)"), "{text}");
        assert!(text.contains("Catalogue: not chosen yet"), "{text}");
        assert!(text.contains("Server: lensflow.in"), "{text}");
    }
}
