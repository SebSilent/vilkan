//! First native frame on screen: render the fullscreen-triangle gradient
//! offscreen and present it to a real swapchain window. No Proton, no D3D12.
//!
//!   VILKAN_VK_FIXTURES=<dir with vs_ids.spv ps_flat.spv> \
//!       cargo run -p vilkan-vk --example present --features present
//!
//! Skips (exits 0) when the fixtures are absent.

use std::sync::Arc;
use vilkan_vk::Context;
use vilkan_vk::graphics::RenderTarget;
use vilkan_vk::present::Swapchain;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

fn fixture(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("VILKAN_VK_FIXTURES")?;
    std::fs::read(std::path::Path::new(&dir).join(name)).ok()
}

struct App {
    window: Option<Arc<Window>>,
    swapchain: Option<Swapchain<'static>>,
    rt: Option<RenderTarget<'static>>,
    vs: Vec<u8>,
    ps: Vec<u8>,
    presented: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = Window::default_attributes().with_title("VIlkan — first native frame").with_inner_size(winit::dpi::LogicalSize::new(512u32, 512u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        // The Context is leaked to 'static (it lives for the process); the
        // RenderTarget and Swapchain borrow it and are owned by the app, dropped
        // before exit. The offscreen gradient is rendered once and blitted each
        // redraw.
        let ctx: &'static Context = Box::leak(Box::new(Context::new().expect("Vulkan device")));
        let rt = RenderTarget::new(ctx, 64, 64).expect("render target");
        let _ = rt.draw_and_readback(&self.vs, &self.ps, [0.0, 0.0, 0.0, 0.0]);
        let sc = Swapchain::new(ctx, window.as_ref(), 512, 512).expect("swapchain");
        self.rt = Some(rt);
        self.swapchain = Some(sc);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let (Some(sc), Some(rt)) = (&self.swapchain, &self.rt) {
                    if let Err(e) = sc.present_blit(rt) {
                        eprintln!("present: {e:#}");
                    } else if !self.presented {
                        eprintln!("first native frame presented ✓");
                        self.presented = true;
                    }
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
    let Some(vs) = fixture("vs_ids.spv") else { eprintln!("skip: vs_ids.spv fixture absent"); return };
    let Some(ps) = fixture("ps_flat.spv") else { eprintln!("skip: ps_flat.spv fixture absent"); return };
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App { window: None, swapchain: None, rt: None, vs, ps, presented: false };
    event_loop.run_app(&mut app).expect("run");
}