//! First native frame on screen, with no D3D12 and no Wine-emulated DXVK.
//!
//! Two paths, chosen by whether SPIR-V fixtures are available:
//!
//!   * fixtures present  - render the fullscreen-triangle gradient offscreen
//!     (vilkan_vk::graphics::RenderTarget) and blit it to the swapchain.
//!   * fixtures absent   - present a solid colour straight into the swapchain
//!     image. No pipeline, no render pass, no SPIR-V at all.
//!
//!   cargo run -p vilkan-vk --example present --features present
//!       [VILKAN_VK_FIXTURES=<dir with vs_ids.spv ps_flat.spv>]
//!       [VILKAN_VK_COLOR=0.2,0.6,1.0]   clear colour, 0..1 floats
//!       [VILKAN_VK_FRAMES=120]          exit after N presented frames
//!       [VILKAN_VK_PPM=/tmp/vkframe.ppm] where to write the readback frame
//!
//! The clear path is the visible Vulkan milestone: it proves the instance,
//! surface, swapchain and command submission of vilkan-vk really drive a GPU.
//! The first clear frame also reads itself back *inside Vulkan* (image ->
//! host-visible buffer copy in the same submission) and prints the numbers, so
//! the proof does not depend on screen capture, which on this Wayland box is
//! unreliable.

use std::sync::Arc;
use vilkan_vk::graphics::RenderTarget;
use vilkan_vk::present::Swapchain;
use vilkan_vk::Context;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

fn fixture(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("VILKAN_VK_FIXTURES")?;
    std::fs::read(std::path::Path::new(&dir).join(name)).ok()
}

/// Clear colour as 0..1 floats, from VILKAN_VK_COLOR="r,g,b" (alpha 1).
fn color() -> [f32; 4] {
    let mut c = [0.20, 0.60, 1.00, 1.00];
    if let Some(s) = std::env::var_os("VILKAN_VK_COLOR") {
        for (i, part) in s.to_string_lossy().split(',').take(3).enumerate() {
            if let Ok(v) = part.trim().parse::<f32>() { c[i] = v; }
        }
    }
    c
}

fn frames_limit() -> Option<u32> {
    std::env::var("VILKAN_VK_FRAMES").ok().and_then(|s| s.trim().parse().ok())
}

/// Where the readback PPM goes. A path the *Linux* side can see: the exe runs
/// under wine but its own filesystem writes are real Unix writes, so this is a
/// file we can then read with plain tools.
fn ppm_path() -> Option<std::path::PathBuf> {
    std::env::var_os("VILKAN_VK_PPM").map(std::path::PathBuf::from)
        .or_else(|| Some(std::path::PathBuf::from("vkframe.ppm")))
}

/// Short form of a readback's distinct colours, for the log line.
fn bgra_list(v: &[[u8; 4]]) -> String {
    v.iter().take(6).map(|q| format!("[{},{},{},{}]", q[0], q[1], q[2], q[3])).collect::<Vec<_>>().join(" ")
}

struct App {
    window: Option<Arc<Window>>,
    swapchain: Option<Swapchain<'static>>,
    /// Some => blit the offscreen gradient; None => clear-colour path.
    rt: Option<RenderTarget<'static>>,
    vs: Vec<u8>,
    ps: Vec<u8>,
    color: [f32; 4],
    presented: u32,
    limit: Option<u32>,
    ppm: Option<std::path::PathBuf>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = Window::default_attributes()
            .with_title("VIlkan - first native frame")
            .with_inner_size(LogicalSize::new(512u32, 512u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        // The Context is leaked to 'static: it lives for the whole process and
        // the RenderTarget/Swapchain borrow it, dropping before exit.
        let ctx: &'static Context = match Context::new() {
            Ok(c) => Box::leak(Box::new(c)),
            Err(e) => { eprintln!("no Vulkan device: {e:#}"); event_loop.exit(); return; }
        };
        let rt = if self.vs.is_empty() || self.ps.is_empty() {
            eprintln!("clear-colour path (no SPIR-V fixtures): presenting {:?}", &self.color[..3]);
            None
        } else {
            let rt = RenderTarget::new(ctx, 64, 64).expect("render target");
            let _ = rt.draw_and_readback(&self.vs, &self.ps, [0.0, 0.0, 0.0, 0.0]);
            eprintln!("gradient path: offscreen render target read back, now presenting");
            Some(rt)
        };
        let sc = Swapchain::new(ctx, window.as_ref(), 512, 512).expect("swapchain");
        // ash 0.38's vk::Format has no Debug impl; print the raw enum value.
        eprintln!("swapchain: {}x{} format raw={}", sc.extent.width, sc.extent.height, sc.format.as_raw());
        self.rt = rt;
        self.swapchain = Some(sc);
        self.window = Some(window);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => _event_loop.exit(),
            WindowEvent::KeyboardInput { .. } => _event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // First frame of the clear path reads itself back inside
                // Vulkan; later frames take the plain path. Scoped so the
                // immutable borrow of `self.swapchain` ends before we touch
                // `self.presented`.
                let outcome = {
                    let Some(sc) = &self.swapchain else { return };
                    let first = self.presented == 0;
                    match (&self.rt, first) {
                        (Some(rt), _) => sc.present_blit(rt).map(|_| None),
                        (None, true) => sc.present_clear_readback(self.color, self.ppm.as_deref()).map(Some),
                        (None, false) => sc.present_clear(self.color).map(|_| None),
                    }
                };
                let rb = match outcome {
                    Ok(rb) => rb,
                    Err(e) => { eprintln!("present failed: {e:#}"); _event_loop.exit(); return; }
                };
                let (w, h) = match &self.swapchain { Some(sc) => (sc.extent.width, sc.extent.height), None => (0, 0) };
                self.presented += 1;
                if self.presented == 1 {
                    eprintln!("first native frame presented ({w}x{h}, no D3D12)");
                    if let Some(rb) = &rb {
                        // The numbers, not a screenshot: what the GPU actually
                        // wrote into the buffer we copied it to.
                        eprintln!(
                            "readback: {}x{} row_length={}B row_pitch={:?} format_raw={} packed={}B",
                            rb.width, rb.height, rb.row_length, rb.row_pitch, rb.format.as_raw(), rb.packed.len()
                        );
                        let d = rb.distinct_bgra();
                        eprintln!("readback distinct BGRA quads: {} -> {}", d.len(), bgra_list(&d));
                        for (x, y) in [(0u32, 0u32), (w / 2, h / 2), (w - 1, h - 1)] {
                            if let Some(q) = rb.sample_bgra(x, y) {
                                eprintln!("readback pixel ({x},{y}) BGRA={:?} RGBA=({},{},{})",
                                    q, q[2], q[1], q[0]);
                            }
                        }
                        if let Some(p) = &self.ppm {
                            eprintln!("readback PPM written to {}", p.display());
                        }
                    }
                }
                if self.limit.is_some_and(|n| self.presented >= n) {
                    eprintln!("{} frames presented; exiting", self.presented);
                    _event_loop.exit();
                    return;
                }
                if let Some(win) = &self.window { win.request_redraw(); }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window { w.request_redraw(); }
    }
}

fn main() {
    let vs = fixture("vs_ids.spv").unwrap_or_default();
    let ps = fixture("ps_flat.spv").unwrap_or_default();
    if vs.is_empty() || ps.is_empty() {
        eprintln!("SPIR-V fixtures absent -> fixture-free clear-colour milestone");
    }
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        window: None,
        swapchain: None,
        rt: None,
        vs,
        ps,
        color: color(),
        presented: 0,
        limit: frames_limit(),
        ppm: ppm_path(),
    };
    event_loop.run_app(&mut app).expect("run");
}
