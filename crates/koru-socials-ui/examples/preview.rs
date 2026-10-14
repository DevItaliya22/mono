// SPDX-License-Identifier: MPL-2.0

//! Export the actual native draw path with synthetic data and no Store access.
//! cargo run -p koru-socials-ui --example preview --features no-reader -- composer output.bmp
#[cfg(not(feature = "no-reader"))]
fn main() {
    eprintln!("Run this visual fixture with --features no-reader");
    std::process::exit(2);
}

#[cfg(feature = "no-reader")]
fn main() {
    use koru_draw::{geometry::Rect, theme};
    use vello::{AaConfig, AaSupport, Renderer, RendererOptions, kurbo::Affine, wgpu};
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("composer");
    let output = args.get(2).expect("output.bmp is required");
    let width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(800);
    assert!(width > 0 && height > 0 && width <= 4096 && height <= 4096);
    let mut surface = koru_socials_ui::preview_fixture(mode);
    surface.set_viewport(Rect::new(0., 0., width as f32, height as f32));
    let mut scene = vello::Scene::new();
    surface.draw(&mut scene, Affine::IDENTITY);
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Socials offline preview"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::default(),
    }))
    .expect("GPU device");
    let mut renderer = Renderer::new(
        &device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .expect("renderer");
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer
        .render_to_texture(
            &device,
            &queue,
            &scene,
            &texture.create_view(&Default::default()),
            &vello::RenderParams {
                base_color: theme::p().bg,
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        )
        .expect("render");
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map"));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("readback");
    let bytes = slice.get_mapped_range();
    // Uncompressed 32-bit BMP: portable, and no additional image dependency.
    let pixels = width * height * 4;
    let mut bmp = Vec::with_capacity((54 + pixels) as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54 + pixels).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&pixels.to_le_bytes());
    bmp.extend_from_slice(&[0; 16]);
    for row in 0..height {
        for col in 0..width {
            let i = (row * stride + col * 4) as usize;
            bmp.extend_from_slice(&[bytes[i + 2], bytes[i + 1], bytes[i], 255]);
        }
    }
    std::fs::write(output, bmp).expect("write preview");
    println!("Rendered {mode}: {width}x{height}");
}

#[cfg(feature = "no-reader")]
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::{
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };
    struct Nudge(std::thread::Thread);
    impl Wake for Nudge {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Nudge(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(1)),
        }
    }
}
