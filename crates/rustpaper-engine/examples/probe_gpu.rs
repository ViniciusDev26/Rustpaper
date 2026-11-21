// `cargo run --example probe_gpu`
// Step by step until the GPU is "opened": Instance -> Adapter -> Device + Queue.

fn main() {
    // The Instance is wgpu's entry point: it knows the system's backends.
    let instance = wgpu::Instance::default();

    // 1) PICK a GPU. Instead of listing them all, we ask wgpu to PICK the
    //    best one via request_adapter. HighPerformance makes it prefer the
    //    dedicated GPU (the RTX) over the software one (llvmpipe). It returns a
    //    Future -> block_on waits; and a Result -> unwrap() takes the Ok or panics.
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None, // we do not have a window/surface yet
    }))
    .unwrap();

    let info = adapter.get_info();
    println!(
        "Adapter chosen: {}  [{:?}, {:?}]",
        info.name, info.backend, info.device_type
    );

    // 2) OPEN the device + queue on that GPU. This is where we actually "switch on" the card:
    //    - Device: creates resources (buffers, textures, shaders, pipelines).
    //    - Queue:  sends commands/work for the GPU to execute.
    //    The descriptor asks for limits/features; default() = the minimum guaranteed on
    //    any GPU (enough for now).
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    println!("Device opened successfully.");
    // `_queue` with an underscore: we do not use the queue yet, and the underscore tells
    // the compiler "I know I did not use it, do not warn me". (Without it, unused-variable warning.)
    let _ = &queue;

    // Peek at a real GPU limit just to prove the device knows the hardware.
    let limits = device.limits();
    println!(
        "max_texture_dimension_2d = {}",
        limits.max_texture_dimension_2d
    );
}
