//! The compiler Direct3D 12 makes the shaders with, on Windows: DXC, Microsoft's newer one, where
//! the app came with it, else FXC, the one Windows has.
//!
//! DXC is the pair `dxcompiler.dll` and `dxil.dll` (which signs what the first makes) beside the
//! app's .exe, where the Windows packages put them. On an Intel GPU it made the canvases' pipelines
//! in half the time FXC's took, and the driver crashed on FXC's. A copy of the app without them
//! still runs, on FXC. A DXC found elsewhere on the PATH, as wgpu would take one, is never used: an
//! older one there (an SDK's) can't make our shaders, and nothing draws.

use std::path::Path;
use std::sync::OnceLock;

const DXC: &str = "dxcompiler.dll";
const DXIL: &str = "dxil.dll";
/// The oldest DXC wgpu makes shaders with.
const OLDEST: [u32; 4] = [1, 8, 2502, 0];

/// `desc` with Direct3D 12's compiler chosen, where it has Direct3D 12.
pub(crate) fn with_compiler(mut desc: wgpu::InstanceDescriptor) -> wgpu::InstanceDescriptor {
    if desc.backends.contains(wgpu::Backends::DX12) {
        desc.backend_options.dx12.shader_compiler = dx12_compiler();
    }
    desc
}

/// The compiler for this run: chosen as the first instance with Direct3D 12 is made, and logged.
fn dx12_compiler() -> wgpu::Dx12Compiler {
    static CHOSEN: OnceLock<wgpu::Dx12Compiler> = OnceLock::new();
    CHOSEN
        .get_or_init(|| {
            let exe = std::env::current_exe().ok();
            let (compiler, said) = choose(exe.as_deref().and_then(Path::parent));
            log::info!("Direct3D 12's shader compiler: {said}");
            compiler
        })
        .clone()
}

/// DXC from `dir` if both its files are there, it is new enough and Direct3D 12 starts with it,
/// else FXC; and which, for the log.
fn choose(dir: Option<&Path>) -> (wgpu::Dx12Compiler, String) {
    let fxc = wgpu::Dx12Compiler::Fxc;
    let Some(dir) = dir else {
        return (fxc, "FXC (the app's folder unknown)".to_owned());
    };
    let missing: Vec<&str> = [DXC, DXIL].into_iter().filter(|name| !dir.join(name).is_file()).collect();
    if !missing.is_empty() {
        return (fxc, format!("FXC ({} not beside the app)", missing.join(" and ")));
    }
    let path = dir.join(DXC);
    let version = file_version(&path);
    let named = match version {
        Some(v) => format!("DXC {}", v.map(|n| n.to_string()).join(".")),
        None => "DXC".to_owned(),
    };
    if version.is_some_and(|v| v < OLDEST) {
        return (fxc, format!("FXC (the {named} beside the app is too old)"));
    }
    let dxc = wgpu::Dx12Compiler::DynamicDxc { dxc_path: path.to_string_lossy().into_owned() };
    if starts(&dxc) {
        (dxc, format!("{named} ({})", path.display()))
    } else {
        (fxc, format!("FXC (Direct3D 12 didn't start with the {named} beside the app)"))
    }
}

/// Whether Direct3D 12 starts with `compiler`: wgpu leaves it out of an instance whose compiler
/// doesn't load (a damaged DLL, or one for another kind of PC), and the app would go without it.
fn starts(compiler: &wgpu::Dx12Compiler) -> bool {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    desc.backend_options.dx12.shader_compiler = compiler.clone();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| has_dx12(&wgpu::Instance::new(desc)))).unwrap_or(false)
}

#[allow(unsafe_code)]
fn has_dx12(instance: &wgpu::Instance) -> bool {
    // SAFETY: only whether the instance has Direct3D 12 is asked; what it gives isn't used.
    unsafe { instance.as_hal::<wgpu::hal::api::Dx12>() }.is_some()
}

/// The version a Windows DLL gives itself (the numbers of its VS_FIXEDFILEINFO, among its resources
/// near the file's end).
fn file_version(path: &Path) -> Option<[u32; 4]> {
    use std::io::{Read, Seek, SeekFrom};
    // DXC's is 124 KB from the end, before the relocations and the signature
    const TAIL: u64 = 1024 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    // the signature 0xFEEF04BD then the structure's version 1.0, both little-endian
    let at = tail.windows(8).position(|w| w == [0xBD, 0x04, 0xEF, 0xFE, 0x00, 0x00, 0x01, 0x00])?;
    let word = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(tail.get(at + i..at + i + 4)?.try_into().ok()?)) };
    let (high, low) = (word(8)?, word(12)?);
    Some([high >> 16, high & 0xFFFF, low >> 16, low & 0xFFFF])
}

#[cfg(test)]
mod tests {
    use super::{choose, file_version};

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("procraft-dxc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A DLL's bytes with a version resource giving `version`.
    fn dll(version: [u16; 4]) -> Vec<u8> {
        let [a, b, c, d] = version.map(u32::from);
        let mut bytes = b"MZ not a real DLL".to_vec();
        bytes.extend([0xBD, 0x04, 0xEF, 0xFE, 0x00, 0x00, 0x01, 0x00]);
        bytes.extend(((a << 16) | b).to_le_bytes());
        bytes.extend(((c << 16) | d).to_le_bytes());
        bytes.extend([0; 40]);
        bytes
    }

    #[test]
    fn a_copy_without_both_dlls_takes_fxc() {
        let dir = folder("none");
        assert!(matches!(choose(None).0, wgpu::Dx12Compiler::Fxc));
        let (compiler, said) = choose(Some(&dir));
        assert!(matches!(compiler, wgpu::Dx12Compiler::Fxc));
        assert_eq!(said, "FXC (dxcompiler.dll and dxil.dll not beside the app)");
        std::fs::write(dir.join("dxcompiler.dll"), dll([1, 9, 2609, 5])).unwrap();
        let (compiler, said) = choose(Some(&dir));
        assert!(matches!(compiler, wgpu::Dx12Compiler::Fxc));
        assert_eq!(said, "FXC (dxil.dll not beside the app)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dxc_that_doesnt_load_or_is_too_old_gives_way_to_fxc() {
        let dir = folder("broken");
        std::fs::write(dir.join("dxil.dll"), b"").unwrap();
        std::fs::write(dir.join("dxcompiler.dll"), dll([1, 7, 2308, 16])).unwrap();
        let (compiler, said) = choose(Some(&dir));
        assert!(matches!(compiler, wgpu::Dx12Compiler::Fxc));
        assert_eq!(said, "FXC (the DXC 1.7.2308.16 beside the app is too old)");
        std::fs::write(dir.join("dxcompiler.dll"), dll([1, 9, 2609, 5])).unwrap();
        let (compiler, said) = choose(Some(&dir));
        assert!(matches!(compiler, wgpu::Dx12Compiler::Fxc));
        assert_eq!(said, "FXC (Direct3D 12 didn't start with the DXC 1.9.2609.5 beside the app)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dlls_version_is_read_from_its_resources() {
        let dir = folder("version");
        let path = dir.join("dxcompiler.dll");
        let mut bytes = vec![0x55; 2 * 1024 * 1024];
        bytes.extend(dll([1, 9, 2609, 5]));
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(file_version(&path), Some([1, 9, 2609, 5]));
        std::fs::write(&path, b"MZ").unwrap();
        assert_eq!(file_version(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
