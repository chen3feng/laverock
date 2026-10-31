//! Which ONNX execution provider the sherpa-onnx engines ask for.
//!
//! The default build links the static, CPU-only runtime. Building with
//! `--no-default-features --features onnx-cuda` links the shared runtime that
//! ships the CUDA provider instead, and the engines then ask for CUDA.
//! `LAVEROCK_ONNX_PROVIDER=cpu` switches back to the processor without a
//! rebuild; a provider that fails to start falls back to the CPU as well, so a
//! machine with no usable GPU still synthesizes.

/// Environment variable that overrides the provider (`cpu`, `cuda`, …).
pub const ENV: &str = "LAVEROCK_ONNX_PROVIDER";

/// Provider to request from sherpa-onnx.
///
/// Only a CUDA build honours a GPU request: the static runtime on a CPU-only
/// build has no such provider and would abort during session creation. A CUDA
/// build falls back to the CPU when the machine cannot actually run CUDA,
/// because a failed provider start terminates the process instead of returning
/// an error.
pub fn requested() -> String {
    if !cfg!(feature = "onnx-cuda") {
        return "cpu".into();
    }
    let provider = match std::env::var(ENV) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_ascii_lowercase(),
        _ => "cuda".into(),
    };
    if provider == "cuda" && !gpu_ready() {
        tracing::warn!("CUDA was requested but is not usable here; using the CPU");
        return "cpu".into();
    }
    provider
}

/// Whether the CUDA provider can start on this machine. The Windows runtime
/// needs a driver with a device, plus cuDNN 9 as a separate DLL.
#[cfg(all(feature = "onnx-cuda", windows))]
fn gpu_ready() -> bool {
    use std::ffi::c_int;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

    type CuInit = unsafe extern "system" fn(u32) -> c_int;
    type CuDeviceGetCount = unsafe extern "system" fn(*mut c_int) -> c_int;

    // SAFETY: plain driver entry points with the signatures CUDA documents;
    // both libraries stay loaded for the life of the process.
    unsafe {
        let cuda = LoadLibraryA(c"nvcuda.dll".as_ptr().cast());
        if cuda.is_null() {
            return false;
        }
        let init: Option<CuInit> =
            std::mem::transmute(GetProcAddress(cuda, c"cuInit".as_ptr().cast()));
        let count: Option<CuDeviceGetCount> =
            std::mem::transmute(GetProcAddress(cuda, c"cuDeviceGetCount".as_ptr().cast()));
        let (Some(init), Some(count)) = (init, count) else {
            return false;
        };
        let mut devices: c_int = 0;
        if init(0) != 0 || count(&mut devices) != 0 || devices == 0 {
            return false;
        }
        !LoadLibraryA(c"cudnn64_9.dll".as_ptr().cast()).is_null()
    }
}

/// Elsewhere, trust the build: only a CUDA build asks for CUDA in the first place.
#[cfg(all(feature = "onnx-cuda", not(windows)))]
fn gpu_ready() -> bool {
    true
}

/// No CUDA runtime is linked, so nothing can be ready; `requested` never
/// reaches the check.
#[cfg(not(feature = "onnx-cuda"))]
fn gpu_ready() -> bool {
    false
}

/// Build an ONNX object with the requested provider, retrying on the CPU when
/// the GPU provider cannot start.
pub fn with_fallback<T>(engine: &str, mut create: impl FnMut(&str) -> Option<T>) -> Option<T> {
    let provider = requested();
    match create(&provider) {
        Some(created) => {
            tracing::info!(engine, provider = %provider, "ONNX session ready");
            Some(created)
        }
        None if provider != "cpu" => {
            tracing::warn!(
                engine,
                provider = %provider,
                "ONNX provider unavailable; falling back to the CPU"
            );
            create("cpu")
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the CUDA build there is nothing to ask for but the CPU,
    /// whatever the environment says.
    #[test]
    fn cpu_build_ignores_the_environment() {
        if cfg!(feature = "onnx-cuda") {
            return;
        }
        std::env::set_var(ENV, "cuda");
        assert_eq!(requested(), "cpu");
        std::env::remove_var(ENV);
    }
}
