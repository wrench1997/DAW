//! Optional genuine WGPU offscreen readback for the headless input tests.
//! Enabled only by CITRUS_UI_CAPTURE_DIR; never creates a window/display/surface.

use std::{
    future::Future,
    io::Write,
    path::PathBuf,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use eframe::{egui_wgpu, wgpu};

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn wait_for<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "offscreen adapter/device timed out"
                );
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
    }
}

pub(super) struct OffscreenCapture {
    directory: PathBuf,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
}

impl OffscreenCapture {
    pub(super) fn from_env() -> Option<Self> {
        let directory = PathBuf::from(std::env::var_os("CITRUS_UI_CAPTURE_DIR")?);
        assert!(
            directory.is_absolute(),
            "capture directory must be explicit and absolute"
        );
        std::fs::create_dir_all(&directory).unwrap();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = wait_for(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        }))
        .expect("explicit offscreen capture requires a working Vulkan adapter");
        let info = adapter.get_info();
        eprintln!("Offscreen capture adapter: {info:?}; no display or surface");
        std::fs::write(
            directory.join("adapter.txt"),
            format!(
                "{info:?}\nOffscreen WGPU texture readback; no display, surface or native window.\n"
            ),
        )
        .unwrap();
        let (device, queue) = wait_for(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("offscreen Vulkan device");
        let renderer = egui_wgpu::Renderer::new(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            egui_wgpu::RendererOptions::PREDICTABLE,
        );
        Some(Self {
            directory,
            device,
            queue,
            renderer,
        })
    }

    pub(super) fn process(
        &mut self,
        ctx: &egui::Context,
        output: &egui::FullOutput,
        name: Option<&str>,
    ) {
        for (id, delta) in &output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        if let Some(name) = name {
            self.capture(ctx, output, name);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }

    fn capture(&mut self, ctx: &egui::Context, output: &egui::FullOutput, name: &str) {
        let size = ctx.content_rect().size() * output.pixels_per_point;
        let width = size.x.round() as u32;
        let height = size.y.round() as u32;
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let paint_jobs = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("actual-app-ui-offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row_bytes = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("actual-app-ui-readback"),
            size: u64::from(row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut commands = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("actual-app-ui-render"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &paint_jobs, &screen);
        }
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        commands.push(encoder.finish());
        self.queue.submit(commands);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .expect("offscreen GPU readback");
        receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("readback callback")
            .expect("mapped image");
        let bytes = readback.slice(..).get_mapped_range();
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(self.directory.join(format!("{name}.ppm"))).unwrap(),
        );
        write!(file, "P6\n{width} {height}\n255\n").unwrap();
        for row in bytes.chunks_exact(row_bytes as usize) {
            for pixel in row[..(width * 4) as usize].as_chunks::<4>().0 {
                file.write_all(&pixel[..3]).unwrap();
            }
        }
        file.flush().unwrap();
        drop(bytes);
        readback.unmap();
        eprintln!("Genuine offscreen app render: {name}.ppm ({width} × {height})");
    }
}
