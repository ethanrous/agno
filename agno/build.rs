//! Build script for the agno crate.
//!
//! When the `gpu` feature is enabled, this compiles the GPU kernels crate to
//! SPIR-V using spirv-builder. The SPIR-V binary is then loaded directly by
//! wgpu at runtime.

fn main() {
    #[cfg(feature = "gpu")]
    {
        use spirv_builder::{SpirvBuilder, SpirvMetadata};
        use std::fs;
        use std::path::PathBuf;

        let kernel_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("agno-gpu-kernels");

        // Compile Rust to SPIR-V
        let result = SpirvBuilder::new(kernel_crate, "spirv-unknown-vulkan1.1")
            .spirv_metadata(SpirvMetadata::Full)
            .build()
            .expect("Failed to compile GPU kernels to SPIR-V");

        let spv_path = result.module.unwrap_single();
        println!("cargo:rerun-if-changed={}", spv_path.display());

        // Read the SPIR-V binary
        let spv_bytes = fs::read(&spv_path).expect("Failed to read SPIR-V file");

        // Write SPIR-V to output directory
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let dest = out_dir.join("gpu_kernels.spv");
        fs::write(&dest, &spv_bytes).expect("Failed to write SPIR-V file");

        // Set environment variable for include_bytes! in GPU modules
        println!("cargo:rustc-env=GPU_KERNELS_SPV_PATH={}", dest.display());
    }
}
