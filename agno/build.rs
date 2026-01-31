//! Build script for the agno crate.
//!
//! When the `gpu` feature is enabled, this:
//! 1. Compiles the GPU kernels crate to SPIR-V using spirv-builder
//! 2. Converts the SPIR-V to WGSL using naga for cross-platform compatibility

fn main() {
    #[cfg(feature = "gpu")]
    {
        use naga::back::wgsl;
        use naga::front::spv;
        use naga::valid::{Capabilities, ValidationFlags, Validator};
        use spirv_builder::{MetadataPrintout, SpirvBuilder};
        use std::fs;
        use std::path::PathBuf;

        let kernel_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("agno-gpu-kernels");

        // Step 1: Compile Rust to SPIR-V
        let result = SpirvBuilder::new(kernel_crate, "spirv-unknown-vulkan1.1")
            .print_metadata(MetadataPrintout::Full)
            .build()
            .expect("Failed to compile GPU kernels to SPIR-V");

        let spv_path = result.module.unwrap_single();
        println!("cargo:rerun-if-changed={}", spv_path.display());

        // Step 2: Read the SPIR-V binary
        let spv_bytes = fs::read(&spv_path).expect("Failed to read SPIR-V file");

        // Step 3: Parse SPIR-V with naga
        let options = spv::Options {
            adjust_coordinate_space: false,
            strict_capabilities: false,
            block_ctx_dump_prefix: None,
        };
        let module = spv::parse_u8_slice(&spv_bytes, &options)
            .expect("Failed to parse SPIR-V with naga");

        // Step 4: Validate the module
        let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
        let info = validator
            .validate(&module)
            .expect("SPIR-V module validation failed");

        // Step 5: Convert to WGSL
        let wgsl_source =
            wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
                .expect("Failed to convert SPIR-V to WGSL");

        // Step 6: Write WGSL to output directory
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let wgsl_path = out_dir.join("demosaic_kernel.wgsl");
        fs::write(&wgsl_path, &wgsl_source).expect("Failed to write WGSL file");

        // Set environment variable for include_str! in demosaic_gpu.rs
        println!("cargo:rustc-env=DEMOSAIC_WGSL_PATH={}", wgsl_path.display());
    }
}
