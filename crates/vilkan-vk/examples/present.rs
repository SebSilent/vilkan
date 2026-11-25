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
//!
//! The clear path is the visible Vulkan milestone: it proves the instance,
//! surface, swapchain and command submission of vilkan-vk really drive a GPU.

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
                let Some(sc) = &self.swapchain else { return };
                let r = match &self.rt {
                    Some(rt) => sc.present_blit(rt),
                    None => sc.present_clear(self.color),
                };
                if let Err(e) = r {
                    eprintln!("present failed: {e:#}");
                    _event_loop.exit();
                    return;
                }
                self.presented += 1;
                if self.presented == 1 {
                    eprintln!("first native frame presented ({}x{}, no D3D12)", sc.extent.width, sc.extent.height);
                }
                if self.limit.is_some_and(|n| self.presented >= n) {
                    eprintln!("{} frames presented; exiting", self.presented);
                    _event_loop.exit();
                    return;
                }
                if let Some(w) = &self.window { w.request_redraw(); }
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
    };
    event_loop.run_app(&mut app).expect("run");
}
