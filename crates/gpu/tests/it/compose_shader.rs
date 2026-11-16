//! `compose.wgsl` parses and validates without a GPU (naga, as wgpu does before any driver sees
//! it), so a WGSL error fails on every runner instead of only where an adapter exists.

#[test]
fn the_shader_validates_without_a_gpu() {
    let src = photocraft_gpu::SHADER;
    let module = wgpu::naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{:?}", e));
}

/// Both C3 printing adjustments have their own pass, and the neighbourhood one samples A.
#[test]
fn every_drawn_kernel_has_an_entry_point() {
    let src = photocraft_gpu::SHADER;
    for kernel in photocraft_gpu::plan::Kernel::DRAWN {
        let entry = kernel.entry().expect("a drawn kernel has a fragment entry point");
        assert!(src.contains(entry), "{entry} has no entry point in compose.wgsl");
    }
}
