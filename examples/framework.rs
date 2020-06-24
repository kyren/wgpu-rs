use futures::{
    task::LocalSpawn,
};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
use winit::{
    event::{self, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
};

#[cfg_attr(rustfmt, rustfmt_skip)]
#[allow(unused)]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);

#[allow(dead_code)]
pub fn cast_slice<T>(data: &[T]) -> &[u8] {
    use std::{mem::size_of, slice::from_raw_parts};

    unsafe { from_raw_parts(data.as_ptr() as *const u8, data.len() * size_of::<T>()) }
}

#[allow(dead_code)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

pub trait Example: 'static + Sized {
    fn needed_extensions() -> (wgt::Extensions, wgt::UnsafeExtensions) {
        (wgpu::Extensions::empty(), wgt::UnsafeExtensions::disallow())
    }
    fn init(
        sc_desc: &wgpu::SwapChainDescriptor,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Self;
    fn resize(
        &mut self,
        sc_desc: &wgpu::SwapChainDescriptor,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    );
    fn update(&mut self, event: WindowEvent);
    fn render(
        &mut self,
        frame: &wgpu::SwapChainTexture,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spawner: &impl LocalSpawn,
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run<E: Example>(title: &str) {
    use futures::executor::{block_on, LocalPool};

    let event_loop = EventLoop::new();
    let mut builder = winit::window::WindowBuilder::new();
    builder = builder.with_title(title);
    #[cfg(windows_OFF)] // TODO
    {
        use winit::platform::windows::WindowBuilderExtWindows;
        builder = builder.with_no_redirection_bitmap(true);
    }
    let window = builder.build(&event_loop).unwrap();

    env_logger::init();

    #[cfg(feature = "subscriber")]
    {
        let chrome_tracing_dir = std::env::var("WGPU_CHROME_TRACING");
        wgpu::util::initialize_default_subscriber(chrome_tracing_dir.ok());
    };

    let mut local_pool = LocalPool::new();
    let spawner = local_pool.spawner();

    log::info!("Initializing the surface...");

    let (size, surface, instance, adapter, device, queue) = block_on(async {
        let instance = wgpu::Instance::new(wgpu::BackendBit::PRIMARY);
        let (size, surface) = unsafe {
            let size = window.inner_size();
            let surface = instance.create_surface(&window);
            (size, surface)
        };

        let (needed_extensions, unsafe_extensions) = E::needed_extensions();

        let adapter = instance
            .request_adapter(
                &wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::Default,
                    compatible_surface: Some(&surface),
                },
                unsafe_extensions,
            )
            .await
            .unwrap();

        let adapter_extensions = adapter.extensions();

        let trace_dir = std::env::var("WGPU_TRACE");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    extensions: adapter_extensions & needed_extensions,
                    limits: wgpu::Limits::default(),
                    shader_validation: true,
                },
                trace_dir.ok().as_ref().map(std::path::Path::new),
            )
            .await
            .unwrap();

        (size, surface, instance, adapter, device, queue)
    });

    let mut sc_desc = wgpu::SwapChainDescriptor {
        usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Mailbox,
    };
    let mut swap_chain = device.create_swap_chain(&surface, &sc_desc);

    log::info!("Initializing the example...");
    let mut example = E::init(&sc_desc, &device, &queue);

    let mut last_update_inst = Instant::now();

    log::info!("Entering render loop...");
    event_loop.run(move |event, _, control_flow| {
        let _ = (&instance, &adapter); // force ownership by the closure
        *control_flow = if cfg!(feature = "metal-auto-capture") {
            ControlFlow::Exit
        } else {
            ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10))
        };
        match event {
            event::Event::MainEventsCleared => {
                if last_update_inst.elapsed() > Duration::from_millis(20) {
                    window.request_redraw();
                    last_update_inst = Instant::now();
                }

                local_pool.run_until_stalled();
            }
            event::Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                log::info!("Resizing to {:?}", size);
                sc_desc.width = size.width;
                sc_desc.height = size.height;
                example.resize(&sc_desc, &device, &queue);
            }
            event::Event::WindowEvent { event, .. } => match event {
                WindowEvent::KeyboardInput {
                    input:
                        event::KeyboardInput {
                            virtual_keycode: Some(event::VirtualKeyCode::Escape),
                            state: event::ElementState::Pressed,
                            ..
                        },
                    ..
                }
                | WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                _ => {
                    example.update(event);
                }
            },
            event::Event::RedrawRequested(_) => {
                let frame = match swap_chain.get_next_frame() {
                    Ok(frame) => frame,
                    Err(_) => {
                        swap_chain = device.create_swap_chain(&surface, &sc_desc);
                        swap_chain
                            .get_next_frame()
                            .expect("Failed to acquire next swap chain texture!")
                    }
                };

                example.render(&frame.output, &device, &queue, &spawner);
            }
            _ => {}
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub fn run<E: Example>(title: &str) {
    let event_loop = EventLoop::new();
    let mut builder = winit::window::WindowBuilder::new();
    builder = builder.with_title(title);
    #[cfg(windows_OFF)] // TODO
    {
        use winit::platform::windows::WindowBuilderExtWindows;
        builder = builder.with_no_redirection_bitmap(true);
    }
    let window = builder.build(&event_loop).unwrap();

    use futures::{future::LocalFutureObj, task::SpawnError};
    use winit::platform::web::WindowExtWebSys;

    struct WebSpawner {}
    impl LocalSpawn for WebSpawner {
        fn spawn_local_obj(&self, future: LocalFutureObj<'static, ()>) -> Result<(), SpawnError> {
            Ok(wasm_bindgen_futures::spawn_local(future))
        }
    }
    let spawner = WebSpawner {};

    std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    console_log::init().expect("could not initialize logger");
    // On wasm, append the canvas to the document body
    web_sys::window()
        .and_then(|win| win.document())
        .and_then(|doc| doc.body())
        .and_then(|body| {
            body.append_child(&web_sys::Element::from(window.canvas()))
                .ok()
        })
        .expect("couldn't append canvas to document body");

    wasm_bindgen_futures::spawn_local(async {
        log::info!("Initializing the surface...");

        let instance = wgpu::Instance::new(wgpu::BackendBit::PRIMARY);
        let (size, surface) = unsafe {
            let size = window.inner_size();
            let surface = instance.create_surface(&window);
            (size, surface)
        };

        let (needed_extensions, unsafe_extensions) = E::needed_extensions();

        let adapter = instance
            .request_adapter(
                &wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::Default,
                    compatible_surface: Some(&surface),
                },
                unsafe_extensions,
            )
            .await
            .unwrap();

        let adapter_extensions = adapter.extensions();

        let trace_dir = std::env::var("WGPU_TRACE");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    extensions: adapter_extensions & needed_extensions,
                    limits: wgpu::Limits::default(),
                    shader_validation: true,
                },
                trace_dir.ok().as_ref().map(std::path::Path::new),
            )
            .await
            .unwrap();

        let mut sc_desc = wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            // TODO: Allow srgb unconditionally
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Mailbox,
        };
        let mut swap_chain = device.create_swap_chain(&surface, &sc_desc);

        log::info!("Initializing the example...");
        let mut example = E::init(&sc_desc, &device, &queue);

        log::info!("Entering render loop...");
        event_loop.run(move |event, _, control_flow| {
            let _ = (&instance, &adapter); // force ownership by the closure
            *control_flow = ControlFlow::Exit;
            match event {
                event::Event::MainEventsCleared => {
                    window.request_redraw();
                }
                event::Event::WindowEvent {
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    log::info!("Resizing to {:?}", size);
                    sc_desc.width = size.width;
                    sc_desc.height = size.height;
                    example.resize(&sc_desc, &device, &queue);
                }
                event::Event::WindowEvent { event, .. } => match event {
                    WindowEvent::KeyboardInput {
                        input:
                            event::KeyboardInput {
                                virtual_keycode: Some(event::VirtualKeyCode::Escape),
                                state: event::ElementState::Pressed,
                                ..
                            },
                        ..
                    }
                    | WindowEvent::CloseRequested => {
                        *control_flow = ControlFlow::Exit;
                    }
                    _ => {
                        example.update(event);
                    }
                },
                event::Event::RedrawRequested(_) => {
                    let frame = match swap_chain.get_next_frame() {
                        Ok(frame) => frame,
                        Err(_) => {
                            swap_chain = device.create_swap_chain(&surface, &sc_desc);
                            swap_chain
                                .get_next_frame()
                                .expect("Failed to acquire next swap chain texture!")
                        }
                    };

                    example.render(&frame.output, &device, &queue, &spawner);
                }
                _ => {}
            }
        });
    });
}

// This allows treating the framework as a standalone example,
// thus avoiding listing the example names in `Cargo.toml`.
#[allow(dead_code)]
fn main() {}
