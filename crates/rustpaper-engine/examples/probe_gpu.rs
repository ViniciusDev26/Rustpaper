// `cargo run --example probe_gpu`
// Passo a passo até "abrir" a GPU: Instance -> Adapter -> Device + Queue.

fn main() {
    // A Instance é o ponto de entrada do wgpu: conhece os backends do sistema.
    let instance = wgpu::Instance::default();

    // 1) ESCOLHER uma GPU. Em vez de listar todas, pedimos ao wgpu que ESCOLHA a
    //    melhor via request_adapter. HighPerformance faz ele preferir a GPU
    //    dedicada (a RTX) em vez da de software (llvmpipe). Retorna um Future ->
    //    block_on espera; e um Result -> unwrap() pega o Ok ou estoura.
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None, // ainda não temos janela/surface
    }))
    .unwrap();

    let info = adapter.get_info();
    println!(
        "Adapter escolhido: {}  [{:?}, {:?}]",
        info.name, info.backend, info.device_type
    );

    // 2) ABRIR o device + queue nessa GPU. É aqui que realmente "ligamos" a placa:
    //    - Device: cria recursos (buffers, texturas, shaders, pipelines).
    //    - Queue:  envia comandos/trabalho pra GPU executar.
    //    O descriptor pede limites/recursos; default() = o mínimo garantido em
    //    qualquer GPU (suficiente por enquanto).
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    println!("Device aberto com sucesso.");
    // `_queue` com underscore: ainda não usamos a queue, e o underscore diz ao
    // compilador "sei que não usei, não me avise". (Sem ele, warning de var não usada.)
    let _ = &queue;

    // Espia um limite real da GPU só pra provar que o device conhece o hardware.
    let limits = device.limits();
    println!(
        "max_texture_dimension_2d = {}",
        limits.max_texture_dimension_2d
    );
}
