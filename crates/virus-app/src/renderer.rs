//! Renderer selection.
//!
//! The default is OpenGL (`glow`). On machines without OpenGL 2.0+ (e.g. a Hyper-V VM over RDP,
//! which only has Windows' OpenGL 1.1 software renderer) glow fails to start and the app retries
//! with `wgpu`, which can use Direct3D 12's WARP software rasteriser. The renderer can also be forced
//! with `--renderer glow|wgpu` or the `VIRUS_RENDERER` environment variable (the flag wins).

use eframe::Renderer;

pub const RENDERER_ENV: &str = "VIRUS_RENDERER";

/// The renderer explicitly requested on the command line or in the environment, if any.
pub fn requested(
    args: impl IntoIterator<Item = String>,
    env: Option<String>,
) -> Result<Option<Renderer>, String> {
    let mut args = args.into_iter();
    let mut flag = None;
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--renderer=") {
            flag = Some(value.to_owned());
        } else if arg == "--renderer" {
            flag = Some(
                args.next()
                    .ok_or("--renderer needs a value: glow or wgpu")?,
            );
        }
    }
    match flag.or(env.filter(|v| !v.trim().is_empty())) {
        None => Ok(None),
        Some(name) => match name.trim().to_ascii_lowercase().as_str() {
            "glow" | "gl" | "opengl" => Ok(Some(Renderer::Glow)),
            "wgpu" => Ok(Some(Renderer::Wgpu)),
            other => Err(format!(
                "unknown renderer '{other}' (expected glow or wgpu)"
            )),
        },
    }
}

/// Whether a glow start-up failure should be retried with wgpu.
pub fn is_glow_init_error(err: &eframe::Error) -> bool {
    matches!(
        err,
        eframe::Error::OpenGL(_) | eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..)
    )
}

/// Native options with the renderer set; wgpu skips its GL backend (the same OpenGL 1.1 that
/// stops glow) unless `WGPU_BACKEND` overrides the backend list.
pub fn configure(options: &mut eframe::NativeOptions, renderer: Renderer) {
    options.renderer = renderer;
    if renderer == Renderer::Wgpu {
        options.wgpu_options.supported_backends =
            wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::PRIMARY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_auto() {
        assert_eq!(requested(args(&["app"]), None), Ok(None));
        assert_eq!(requested(args(&["app"]), Some(" ".into())), Ok(None));
    }

    #[test]
    fn flag_overrides_env() {
        assert_eq!(
            requested(args(&["app", "--renderer", "wgpu"]), Some("glow".into())),
            Ok(Some(Renderer::Wgpu))
        );
        assert_eq!(
            requested(args(&["app", "--renderer=GLOW"]), None),
            Ok(Some(Renderer::Glow))
        );
        assert_eq!(
            requested(args(&["app"]), Some("wgpu".into())),
            Ok(Some(Renderer::Wgpu))
        );
    }

    #[test]
    fn rejects_bad_values() {
        assert!(requested(args(&["app", "--renderer"]), None).is_err());
        assert!(requested(args(&["app"]), Some("vulkan".into())).is_err());
    }
}
