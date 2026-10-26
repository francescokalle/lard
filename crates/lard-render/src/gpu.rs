use anyhow::{Context, Result};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, WaylandDisplayHandle,
};
use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::Arc;

/// Shared GPU state. One device/queue is shared by every surface of the app.
pub struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// A borrowed Wayland display handle that wgpu can store in the instance.
///
/// `wgpu` requires the instance to know the display on GLES/Wayland for
/// presentation. The pointer comes from a `Connection` that outlives the
/// instance (the app keeps it alive for the whole run).
struct WaylandDisplay(NonNull<c_void>);

impl fmt::Debug for WaylandDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WaylandDisplay").field(&self.0).finish()
    }
}

// The Wayland display is thread-safe for the operations wgpu performs.
unsafe impl Send for WaylandDisplay {}
unsafe impl Sync for WaylandDisplay {}

impl HasDisplayHandle for WaylandDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.0));
        // SAFETY: the pointer is a live wl_display for the lifetime of the
        // instance; the app owns the Connection.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl Gpu {
    pub fn new(display: Option<NonNull<c_void>>) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: display.map(|display| {
                Box::new(WaylandDisplay(display)) as Box<dyn wgpu::wgt::WgpuHasDisplayHandle>
            }),
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter (Vulkan/OpenGL)")?;

        let info = adapter.get_info();
        tracing::info!(
            backend = ?info.backend,
            device = %info.name,
            "GPU adapter selected"
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lard-device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .context("failed to create wgpu device")?;

        device.on_uncaptured_error(Arc::new(|error| {
            // Also on stderr: headless tools (previews, probes) do not install a
            // tracing subscriber, and a swallowed validation error used to look
            // like a black screen.
            eprintln!("wgpu: {error}");
            tracing::error!(%error, "wgpu validation error");
        }));

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// Create a wgpu surface from raw Wayland handles.
    ///
    /// # Safety
    ///
    /// `display` must be a live `wl_display` and `surface` a live `wl_surface`.
    /// Both must outlive the returned `wgpu::Surface`. The caller is
    /// responsible for dropping the surface *before* destroying the Wayland
    /// objects (see `lard-wayland`'s `SurfaceEntry` field order).
    pub unsafe fn create_wayland_surface(
        &self,
        display: *mut c_void,
        surface: *mut c_void,
    ) -> Result<wgpu::Surface<'static>> {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        };

        let display = NonNull::new(display).context("null wl_display")?;
        let surface = NonNull::new(surface).context("null wl_surface")?;

        let raw_display_handle =
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display.cast()));
        let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface.cast()));

        // SAFETY: upheld by the caller, see the function contract.
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display_handle),
                    raw_window_handle,
                })
        }
        .context("wgpu could not create a Wayland surface")?;

        Ok(surface)
    }

    /// Small helper so callers cannot accidentally create a vertex buffer
    /// without the right usage flags.
    pub(crate) fn create_vertex_buffer(&self, label: &str, len: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: len.max(4),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

/// Convenience for logs/tools.
pub fn fully_qualified_adapter_name(gpu: &Gpu) -> String {
    let info = gpu.adapter.get_info();
    format!("{} ({:?})", info.name, info.backend)
}
