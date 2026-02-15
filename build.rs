fn main() {
    // Windows-specific linking
    #[cfg(windows)]
    {
        // Link required Windows libraries for D3D11
        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=dxguid");

        // User32 for window/message functions
        println!("cargo:rustc-link-lib=user32");

        // GDI32 for graphics functions
        println!("cargo:rustc-link-lib=gdi32");
    }

    // Re-run if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");
}
