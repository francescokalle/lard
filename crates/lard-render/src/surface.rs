//! A `wgpu::Surface` attached to a Wayland surface, configured for a
//! transparent, vsynced, premultiplied-alpha layer or lock surface.

use crate::gpu::Gpu;
use anyhow::Result;
use std::ffi::c_void;

pub struct RenderSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    logical_size: (u32, u32),
    physical_size: (u32, u32),
    scale: f32,
    configured: bool,
    transparent: bool,
}

impl RenderSurface {
    /// # Safety
    ///
    /// The Wayland display/surface must stay alive until this value is dropped.
    /// Drop `RenderSurface` *before* dropping the `wl_surface` (the
    /// `SurfaceEntry` in `lard-wayland` guarantees this by field order).
    pub unsafe fn new(
        gpu: &Gpu,
        display: *mut c_void,
        wl_surface: *mut c_void,
        logical_size: (u32, u32),
        scale: f32,
    ) -> Result<Self> {
        // SAFETY: forwarded to the caller.
        let surface = unsafe { gpu.create_wayland_surface(display, wl_surface)? };

        let caps = surface.get_capabilities(gpu.adapter());
        if caps.formats.is_empty() {
            anyhow::bail!("Wayland surface has no supported wgpu format");
        }

        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);

        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ]
        .into_iter()
        .find(|mode| caps.alpha_modes.contains(mode))
        .unwrap_or(wgpu::CompositeAlphaMode::Opaque);

        let transparent = matches!(
            alpha_mode,
            wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::PostMultiplied
        );

        if !format.is_srgb() {
            tracing::warn!(
                ?format,
                "no sRGB surface format; colors may be slightly darker"
            );
        }

        let scale = scale.max(0.5);
        let physical_size = physical(logical_size, scale);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: physical_size.0,
            height: physical_size.1,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };

        let mut this = Self {
            surface,
            config,
            format,
            alpha_mode,
            logical_size: (0, 0),
            physical_size: (0, 0),
            scale,
            configured: false,
            transparent,
        };
        this.apply_config(gpu, logical_size, scale);
        Ok(this)
    }

    /// Reconfigure for a new surface-local size and/or output scale.
    ///
    /// Returns `true` when the swapchain dimensions actually changed, which is
    /// the only case where the caller must rebuild layout-dependent state.
    pub fn resize(&mut self, gpu: &Gpu, logical_size: (u32, u32), scale: f32) -> bool {
        if logical_size == self.logical_size && (scale - self.scale).abs() < f32::EPSILON {
            return false;
        }
        self.apply_config(gpu, logical_size, scale)
    }

    fn apply_config(&mut self, gpu: &Gpu, logical_size: (u32, u32), scale: f32) -> bool {
        self.scale = scale.max(0.5);
        self.logical_size = logical_size;
        self.physical_size = physical(logical_size, self.scale);

        // A zero-sized surface cannot be configured. Wait for the compositor's
        // first configure.
        if self.physical_size.0 == 0 || self.physical_size.1 == 0 {
            self.configured = false;
            return false;
        }

        self.config.width = self.physical_size.0;
        self.config.height = self.physical_size.1;
        self.surface.configure(gpu.device(), &self.config);
        self.configured = true;
        true
    }

    fn reconfigure(&mut self, gpu: &Gpu) {
        if self.physical_size.0 > 0 && self.physical_size.1 > 0 {
            self.surface.configure(gpu.device(), &self.config);
            self.configured = true;
        }
    }

    /// Acquire the next frame, transparently recovering from a lost swapchain.
    ///
    /// Returns `None` when the frame should be skipped (occluded, timeout,
    /// outdated). This is not an error.
    pub fn acquire(&mut self, gpu: &Gpu) -> Option<wgpu::SurfaceTexture> {
        if !self.configured {
            return None;
        }
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure(gpu);
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                tracing::trace!("surface acquire timeout, skipping frame");
                None
            }
            wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::error!("surface acquire validation error");
                None
            }
        }
    }

    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    #[must_use]
    pub fn physical_size(&self) -> (u32, u32) {
        self.physical_size
    }

    #[must_use]
    pub fn logical_size(&self) -> (u32, u32) {
        self.logical_size
    }

    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    #[must_use]
    pub fn is_transparent(&self) -> bool {
        self.transparent
    }

    #[must_use]
    pub fn alpha_mode(&self) -> wgpu::CompositeAlphaMode {
        self.alpha_mode
    }

    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.configured
    }

    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }
}

#[must_use]
fn physical(logical: (u32, u32), scale: f32) -> (u32, u32) {
    if logical.0 == 0 || logical.1 == 0 {
        return (0, 0);
    }
    (
        ((logical.0 as f32 * scale).round() as u32).max(1),
        ((logical.1 as f32 * scale).round() as u32).max(1),
    )
}
